package snapshot

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/config"
	"github.com/router-for-me/CLIProxyAPI/v7/internal/registry"
	coreauth "github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/auth"
)

const (
	defaultCodexBaseURL       = "https://chatgpt.com/backend-api/codex"
	defaultCodexUserAgent     = "codex-tui/0.135.0 (Mac OS 26.5.0; arm64) iTerm.app/3.6.10 (codex-tui; 0.135.0)"
	defaultCodexResponsesBeta = "responses=v1"
	defaultSessionTTLSeconds  = 3600
	providerCodex             = "codex"
	authKindOAuth             = "oauth"
)

type RuntimeSnapshot struct {
	Version          string                       `json:"version"`
	GeneratedAt      string                       `json:"generated_at"`
	SourceInstanceID string                       `json:"source_instance_id"`
	Listeners        ListenerConfig               `json:"listeners"`
	Routes           RouteConfig                  `json:"routes"`
	Routing          RoutingConfig                `json:"routing"`
	Providers        map[string]ProviderConfig    `json:"providers"`
	ModelAliases     map[string]map[string]string `json:"model_aliases"`
	Models           map[string][]string          `json:"models"`
	AuthPool         []AuthRecord                 `json:"auth_pool"`
	Network          NetworkConfig                `json:"network"`
	UsageQueue       UsageQueueConfig             `json:"usage_queue"`
	FeatureFlags     map[string]bool              `json:"feature_flags"`
}

type ListenerConfig struct {
	PublicHTTP string `json:"public_http"`
}

type RouteConfig struct {
	Responses       bool `json:"responses"`
	ChatCompletions bool `json:"chat_completions"`
	Messages        bool `json:"messages"`
}

type RoutingConfig struct {
	Strategy          string `json:"strategy"`
	SessionAffinity   bool   `json:"session_affinity"`
	SessionTTLSeconds uint64 `json:"session_ttl_seconds"`
}

type ProviderConfig struct {
	Enabled bool `json:"enabled"`
}

type AuthRecord struct {
	ID             string        `json:"id"`
	Provider       string        `json:"provider"`
	AuthKind       string        `json:"auth_kind"`
	UsageSource    string        `json:"usage_source,omitempty"`
	Priority       int           `json:"priority"`
	Enabled        bool          `json:"enabled"`
	SupportsModels []string      `json:"supports_models"`
	Labels         []string      `json:"labels"`
	Execution      AuthExecution `json:"execution"`
	CooldownUntil  *string       `json:"cooldown_until"`
}

type AuthExecution struct {
	Codex *CodexExecution `json:"codex,omitempty"`
}

type CodexExecution struct {
	AccessToken string `json:"access_token"`
	AccountID   string `json:"account_id"`
	BaseURL     string `json:"base_url"`
	UserAgent   string `json:"user_agent"`
	OpenAIBeta  string `json:"openai_beta"`
}

type UsageQueueConfig struct {
	Enabled bool   `json:"enabled"`
	Backend string `json:"backend"`
}

type NetworkConfig struct {
	UpstreamProxy string `json:"upstream_proxy"`
}

func BuildRuntimeSnapshot(cfg *config.Config, authManager *coreauth.Manager, now time.Time) RuntimeSnapshot {
	if now.IsZero() {
		now = time.Now().UTC()
	} else {
		now = now.UTC()
	}

	exportedAuths := buildCodexAuthPool(cfg, authManager, now)
	models := unionModels(exportedAuths)
	modelAliases := buildCodexModelAliases(cfg, models)
	providers := map[string]ProviderConfig{
		providerCodex: {
			Enabled: len(exportedAuths) > 0,
		},
	}

	snapshot := RuntimeSnapshot{
		GeneratedAt:      now.Format(time.RFC3339),
		SourceInstanceID: sourceInstanceID(),
		Listeners: ListenerConfig{
			PublicHTTP: strings.TrimSpace(dataPlanePublicHTTP(cfg)),
		},
		Routes: RouteConfig{
			Responses:       true,
			ChatCompletions: false,
			Messages:        false,
		},
		Routing: RoutingConfig{
			Strategy:          normalizeRoutingStrategy(cfg),
			SessionAffinity:   cfg != nil && cfg.Routing.SessionAffinity,
			SessionTTLSeconds: parseSessionTTLSeconds(cfg),
		},
		Providers: providers,
		ModelAliases: map[string]map[string]string{
			providerCodex: modelAliases,
		},
		Models: map[string][]string{
			providerCodex: models,
		},
		Network: NetworkConfig{
			UpstreamProxy: strings.TrimSpace(proxyURL(cfg)),
		},
		AuthPool:     exportedAuths,
		UsageQueue:   buildUsageQueueConfig(cfg),
		FeatureFlags: map[string]bool{},
	}
	snapshot.Version = buildSnapshotVersion(snapshot)
	return snapshot
}

func buildUsageQueueConfig(cfg *config.Config) UsageQueueConfig {
	if cfg == nil {
		return UsageQueueConfig{}
	}
	enabled := cfg.UsageStatisticsEnabled && (strings.TrimSpace(cfg.RemoteManagement.SecretKey) != "" || cfg.Home.Enabled)
	if !enabled {
		return UsageQueueConfig{}
	}
	return UsageQueueConfig{
		Enabled: true,
		Backend: "redis",
	}
}

func dataPlanePublicHTTP(cfg *config.Config) string {
	if cfg == nil {
		return ""
	}
	return cfg.DataPlane.EffectiveResponsesBaseURL()
}

func proxyURL(cfg *config.Config) string {
	if cfg == nil {
		return ""
	}
	return strings.TrimSpace(cfg.ProxyURL)
}

func buildCodexAuthPool(cfg *config.Config, authManager *coreauth.Manager, now time.Time) []AuthRecord {
	if authManager == nil {
		return []AuthRecord{}
	}

	auths := authManager.List()
	records := make([]AuthRecord, 0, len(auths))
	for _, auth := range auths {
		record, ok := buildCodexAuthRecord(cfg, auth, now)
		if !ok {
			continue
		}
		records = append(records, record)
	}

	sort.Slice(records, func(i, j int) bool {
		if records[i].Priority != records[j].Priority {
			return records[i].Priority > records[j].Priority
		}
		return records[i].ID < records[j].ID
	})
	return records
}

func buildCodexAuthRecord(cfg *config.Config, auth *coreauth.Auth, now time.Time) (AuthRecord, bool) {
	if auth == nil {
		return AuthRecord{}, false
	}
	if !strings.EqualFold(strings.TrimSpace(auth.Provider), providerCodex) {
		return AuthRecord{}, false
	}
	authKind := strings.ToLower(strings.TrimSpace(stringAttr(auth.Attributes, "auth_kind")))
	if authKind != authKindOAuth {
		return AuthRecord{}, false
	}
	if auth.Disabled || auth.Status == coreauth.StatusDisabled {
		return AuthRecord{}, false
	}

	accessToken := strings.TrimSpace(anyString(auth.Metadata, "access_token"))
	if accessToken == "" {
		return AuthRecord{}, false
	}

	supportsModels := codexModelsForAuth(auth)
	if len(supportsModels) == 0 {
		return AuthRecord{}, false
	}

	cooldownUntil := authCooldownUntil(auth, now)
	labels := authLabels(auth)
	record := AuthRecord{
		ID:             strings.TrimSpace(auth.ID),
		Provider:       providerCodex,
		AuthKind:       authKindOAuth,
		UsageSource:    usageSourceForAuth(auth),
		Priority:       parsePriority(auth.Attributes),
		Enabled:        true,
		SupportsModels: supportsModels,
		Labels:         labels,
		Execution: AuthExecution{
			Codex: &CodexExecution{
				AccessToken: accessToken,
				AccountID:   strings.TrimSpace(anyString(auth.Metadata, "account_id")),
				BaseURL:     codexBaseURL(auth),
				UserAgent:   codexUserAgent(cfg),
				OpenAIBeta:  defaultCodexResponsesBeta,
			},
		},
		CooldownUntil: cooldownUntil,
	}
	return record, true
}

func usageSourceForAuth(auth *coreauth.Auth) string {
	if auth == nil {
		return ""
	}
	if _, value := auth.AccountInfo(); strings.TrimSpace(value) != "" {
		return strings.TrimSpace(value)
	}
	if auth.Metadata != nil {
		if email := strings.TrimSpace(anyString(auth.Metadata, "email")); email != "" {
			return email
		}
	}
	if auth.Attributes != nil {
		if apiKey := strings.TrimSpace(auth.Attributes["api_key"]); apiKey != "" {
			return apiKey
		}
	}
	return ""
}

func codexModelsForAuth(auth *coreauth.Auth) []string {
	models := codexRegistryModels(planType(auth))
	models = applyExcludedModels(models, auth)
	out := make([]string, 0, len(models))
	seen := make(map[string]struct{}, len(models))
	for _, model := range models {
		if model == nil {
			continue
		}
		id := strings.TrimSpace(model.ID)
		if id == "" {
			continue
		}
		if _, ok := seen[id]; ok {
			continue
		}
		seen[id] = struct{}{}
		out = append(out, id)
	}
	sort.Strings(out)
	return out
}

func codexRegistryModels(plan string) []*registry.ModelInfo {
	switch strings.ToLower(strings.TrimSpace(plan)) {
	case "free":
		return registry.GetCodexFreeModels()
	case "plus":
		return registry.GetCodexPlusModels()
	case "team", "business", "go":
		return registry.GetCodexTeamModels()
	case "pro":
		return registry.GetCodexProModels()
	default:
		return registry.GetCodexProModels()
	}
}

func applyExcludedModels(models []*registry.ModelInfo, auth *coreauth.Auth) []*registry.ModelInfo {
	if len(models) == 0 {
		return nil
	}
	excludedRaw := stringAttr(auth.Attributes, "excluded_models")
	if strings.TrimSpace(excludedRaw) == "" {
		return models
	}
	excluded := make(map[string]struct{})
	for _, item := range strings.Split(excludedRaw, ",") {
		trimmed := strings.ToLower(strings.TrimSpace(item))
		if trimmed != "" {
			excluded[trimmed] = struct{}{}
		}
	}
	if len(excluded) == 0 {
		return models
	}

	out := make([]*registry.ModelInfo, 0, len(models))
	for _, model := range models {
		if model == nil {
			continue
		}
		if _, ok := excluded[strings.ToLower(strings.TrimSpace(model.ID))]; ok {
			continue
		}
		out = append(out, model)
	}
	return out
}

func buildCodexModelAliases(cfg *config.Config, models []string) map[string]string {
	aliases := map[string]string{}
	if cfg == nil || len(cfg.OAuthModelAlias) == 0 {
		return aliases
	}
	available := make(map[string]struct{}, len(models))
	for _, model := range models {
		available[strings.ToLower(strings.TrimSpace(model))] = struct{}{}
	}
	for _, entry := range cfg.OAuthModelAlias[providerCodex] {
		name := strings.TrimSpace(entry.Name)
		alias := strings.TrimSpace(entry.Alias)
		if name == "" || alias == "" || strings.EqualFold(name, alias) {
			continue
		}
		if _, ok := available[strings.ToLower(name)]; !ok {
			continue
		}
		aliases[alias] = name
	}
	return aliases
}

func unionModels(auths []AuthRecord) []string {
	seen := make(map[string]struct{})
	out := make([]string, 0)
	for _, auth := range auths {
		for _, model := range auth.SupportsModels {
			trimmed := strings.TrimSpace(model)
			if trimmed == "" {
				continue
			}
			if _, ok := seen[trimmed]; ok {
				continue
			}
			seen[trimmed] = struct{}{}
			out = append(out, trimmed)
		}
	}
	sort.Strings(out)
	return out
}

func buildSnapshotVersion(snapshot RuntimeSnapshot) string {
	payload := struct {
		Listeners    ListenerConfig               `json:"listeners"`
		Routes       RouteConfig                  `json:"routes"`
		Routing      RoutingConfig                `json:"routing"`
		Providers    map[string]ProviderConfig    `json:"providers"`
		ModelAliases map[string]map[string]string `json:"model_aliases"`
		Models       map[string][]string          `json:"models"`
		AuthPool     []AuthRecord                 `json:"auth_pool"`
		Network      NetworkConfig                `json:"network"`
		UsageQueue   UsageQueueConfig             `json:"usage_queue"`
		FeatureFlags map[string]bool              `json:"feature_flags"`
	}{
		Listeners:    snapshot.Listeners,
		Routes:       snapshot.Routes,
		Routing:      snapshot.Routing,
		Providers:    snapshot.Providers,
		ModelAliases: snapshot.ModelAliases,
		Models:       snapshot.Models,
		AuthPool:     snapshot.AuthPool,
		Network:      snapshot.Network,
		UsageQueue:   snapshot.UsageQueue,
		FeatureFlags: snapshot.FeatureFlags,
	}
	raw, _ := json.Marshal(payload)
	sum := sha256.Sum256(raw)
	return "sha256:" + hex.EncodeToString(sum[:])
}

func sourceInstanceID() string {
	hostname, err := os.Hostname()
	if err != nil {
		return "local-cpa"
	}
	hostname = strings.TrimSpace(hostname)
	if hostname == "" {
		return "local-cpa"
	}
	return hostname
}

func normalizeRoutingStrategy(cfg *config.Config) string {
	if cfg == nil {
		return "round-robin"
	}
	switch strings.ToLower(strings.TrimSpace(cfg.Routing.Strategy)) {
	case "fill-first", "fillfirst", "ff":
		return "fill-first"
	default:
		return "round-robin"
	}
}

func parseSessionTTLSeconds(cfg *config.Config) uint64 {
	if cfg == nil {
		return defaultSessionTTLSeconds
	}
	raw := strings.TrimSpace(cfg.Routing.SessionAffinityTTL)
	if raw == "" {
		return defaultSessionTTLSeconds
	}
	duration, err := time.ParseDuration(raw)
	if err != nil || duration <= 0 {
		return defaultSessionTTLSeconds
	}
	return uint64(duration / time.Second)
}

func parsePriority(attrs map[string]string) int {
	raw := strings.TrimSpace(stringAttr(attrs, "priority"))
	if raw == "" {
		return 0
	}
	priority, err := strconv.Atoi(raw)
	if err != nil {
		return 0
	}
	return priority
}

func authLabels(auth *coreauth.Auth) []string {
	plan := planType(auth)
	if plan == "" {
		return []string{}
	}
	return []string{plan}
}

func planType(auth *coreauth.Auth) string {
	return strings.ToLower(strings.TrimSpace(stringAttr(auth.Attributes, "plan_type")))
}

func authCooldownUntil(auth *coreauth.Auth, now time.Time) *string {
	candidates := []time.Time{
		auth.NextRetryAfter,
		auth.Quota.NextRecoverAt,
	}
	var best time.Time
	for _, candidate := range candidates {
		if candidate.IsZero() {
			continue
		}
		candidate = candidate.UTC()
		if !candidate.After(now) {
			continue
		}
		if best.IsZero() || candidate.After(best) {
			best = candidate
		}
	}
	if best.IsZero() {
		return nil
	}
	formatted := best.Format(time.RFC3339)
	return &formatted
}

func codexBaseURL(auth *coreauth.Auth) string {
	if auth != nil {
		if value := strings.TrimSpace(stringAttr(auth.Attributes, "base_url")); value != "" {
			return value
		}
		if value := strings.TrimSpace(anyString(auth.Metadata, "base_url")); value != "" {
			return value
		}
	}
	return defaultCodexBaseURL
}

func codexUserAgent(cfg *config.Config) string {
	if cfg != nil {
		if value := strings.TrimSpace(cfg.CodexHeaderDefaults.UserAgent); value != "" {
			return value
		}
	}
	return defaultCodexUserAgent
}

func stringAttr(values map[string]string, key string) string {
	if len(values) == 0 {
		return ""
	}
	return values[key]
}

func anyString(values map[string]any, key string) string {
	if len(values) == 0 {
		return ""
	}
	value, ok := values[key]
	if !ok || value == nil {
		return ""
	}
	switch typed := value.(type) {
	case string:
		return typed
	default:
		return ""
	}
}
