package api

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/config"
	"github.com/router-for-me/CLIProxyAPI/v7/internal/redisqueue"
	log "github.com/sirupsen/logrus"
)

const (
	dataPlaneUsageQueuePollCount  = 64
	dataPlaneUsageQueueRetryDelay = time.Second
)

type dataPlaneUsageBridgeConfig struct {
	enabled      bool
	baseURL      string
	authPassword string
}

func (s *Server) reconcileDataPlaneUsageBridge(cfg *config.Config) {
	if s == nil {
		return
	}
	bridgeCfg := resolveDataPlaneUsageBridgeConfig(cfg, s.localPassword)
	if !bridgeCfg.enabled {
		s.stopDataPlaneUsageBridge()
		return
	}

	s.dataPlaneUsageBridgeMu.Lock()
	if s.dataPlaneUsageBridgeCancel != nil &&
		s.dataPlaneUsageBridgeBaseURL == bridgeCfg.baseURL &&
		s.dataPlaneUsageBridgeAuth == bridgeCfg.authPassword {
		s.dataPlaneUsageBridgeMu.Unlock()
		return
	}
	if s.dataPlaneUsageBridgeCancel != nil {
		s.dataPlaneUsageBridgeCancel()
		s.dataPlaneUsageBridgeCancel = nil
		s.dataPlaneUsageBridgeBaseURL = ""
		s.dataPlaneUsageBridgeAuth = ""
	}
	ctx, cancel := context.WithCancel(context.Background())
	s.dataPlaneUsageBridgeCancel = cancel
	s.dataPlaneUsageBridgeBaseURL = bridgeCfg.baseURL
	s.dataPlaneUsageBridgeAuth = bridgeCfg.authPassword
	s.dataPlaneUsageBridgeMu.Unlock()

	go runDataPlaneUsageBridge(ctx, bridgeCfg.baseURL, bridgeCfg.authPassword)
}

func (s *Server) stopDataPlaneUsageBridge() {
	if s == nil {
		return
	}
	s.dataPlaneUsageBridgeMu.Lock()
	cancel := s.dataPlaneUsageBridgeCancel
	s.dataPlaneUsageBridgeCancel = nil
	s.dataPlaneUsageBridgeBaseURL = ""
	s.dataPlaneUsageBridgeAuth = ""
	s.dataPlaneUsageBridgeMu.Unlock()
	if cancel != nil {
		cancel()
	}
}

func dataPlaneUsageBridgeBaseURL(cfg *config.Config) string {
	if cfg == nil || !cfg.UsageStatisticsEnabled {
		return ""
	}
	baseURL := strings.TrimSpace(cfg.DataPlane.EffectiveResponsesBaseURL())
	if baseURL == "" {
		return ""
	}
	if strings.TrimSpace(cfg.RemoteManagement.SecretKey) == "" && !cfg.Home.Enabled {
		return ""
	}
	return baseURL
}

func dataPlaneUsageBridgeAuthPassword(localPassword string) string {
	if password := strings.TrimSpace(localPassword); password != "" {
		return password
	}
	return strings.TrimSpace(os.Getenv("MANAGEMENT_PASSWORD"))
}

func resolveDataPlaneUsageBridgeConfig(cfg *config.Config, localPassword string) dataPlaneUsageBridgeConfig {
	baseURL := dataPlaneUsageBridgeBaseURL(cfg)
	if baseURL == "" {
		return dataPlaneUsageBridgeConfig{}
	}
	return dataPlaneUsageBridgeConfig{
		enabled:      true,
		baseURL:      baseURL,
		authPassword: dataPlaneUsageBridgeAuthPassword(localPassword),
	}
}

func runDataPlaneUsageBridge(ctx context.Context, baseURL string, authPassword string) {
	client := &http.Client{Timeout: 5 * time.Second}

	for {
		if ctx.Err() != nil {
			return
		}

		if errSubscribe := subscribeDataPlaneUsageQueue(ctx, baseURL, authPassword); errSubscribe != nil && ctx.Err() == nil {
			log.Debugf("data-plane usage bridge subscribe failed: %v", errSubscribe)
		}

		if items, errPoll := fetchDataPlaneUsageQueue(ctx, client, baseURL, dataPlaneUsageQueuePollCount); errPoll == nil {
			for _, item := range items {
				redisqueue.Enqueue(item)
			}
		} else if ctx.Err() == nil {
			log.Debugf("data-plane usage bridge fallback poll failed: %v", errPoll)
		}

		timer := time.NewTimer(dataPlaneUsageQueueRetryDelay)
		select {
		case <-ctx.Done():
			if !timer.Stop() {
				<-timer.C
			}
			return
		case <-timer.C:
		}
	}
}

func subscribeDataPlaneUsageQueue(ctx context.Context, baseURL string, authPassword string) error {
	address, errAddr := dataPlaneUsageQueueAddress(baseURL)
	if errAddr != nil {
		return errAddr
	}
	var dialer net.Dialer
	conn, errDial := dialer.DialContext(ctx, "tcp", address)
	if errDial != nil {
		return errDial
	}
	defer conn.Close()

	if deadline, ok := ctx.Deadline(); ok {
		_ = conn.SetDeadline(deadline)
	}
	reader := bufio.NewReader(conn)
	if errWrite := writeBridgeRESPCommand(conn, "AUTH", authPassword); errWrite != nil {
		return errWrite
	}
	if _, errRead := readBridgeRESPSimpleString(reader); errRead != nil {
		return errRead
	}
	if errWrite := writeBridgeRESPCommand(conn, "SUBSCRIBE", "usage"); errWrite != nil {
		return errWrite
	}
	if channel, _, errSub := readBridgeRESPPubSubSubscribe(reader); errSub != nil {
		return errSub
	} else if !strings.EqualFold(channel, "usage") {
		return fmt.Errorf("unexpected data-plane usage channel %q", channel)
	}

	for {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		channel, payload, errMsg := readBridgeRESPPubSubMessage(reader)
		if errMsg != nil {
			return errMsg
		}
		if !strings.EqualFold(channel, "usage") {
			continue
		}
		if isUsageControlPayload(payload) {
			continue
		}
		redisqueue.Enqueue(payload)
	}
}

func fetchDataPlaneUsageQueue(ctx context.Context, client *http.Client, baseURL string, count int) ([]json.RawMessage, error) {
	if client == nil {
		client = http.DefaultClient
	}
	if count <= 0 {
		count = dataPlaneUsageQueuePollCount
	}
	target, errURL := dataPlaneUsageQueueURL(baseURL, count)
	if errURL != nil {
		return nil, errURL
	}
	req, errReq := http.NewRequestWithContext(ctx, http.MethodGet, target, nil)
	if errReq != nil {
		return nil, errReq
	}
	resp, errDo := client.Do(req)
	if errDo != nil {
		return nil, errDo
	}
	defer resp.Body.Close()
	body, errRead := io.ReadAll(resp.Body)
	if errRead != nil {
		return nil, errRead
	}
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("data-plane usage queue status %d: %s", resp.StatusCode, strings.TrimSpace(string(body)))
	}
	var items []json.RawMessage
	if errJSON := json.Unmarshal(body, &items); errJSON != nil {
		return nil, errJSON
	}
	return items, nil
}

func dataPlaneUsageQueueURL(baseURL string, count int) (string, error) {
	parsed, errParse := url.Parse(strings.TrimSpace(baseURL))
	if errParse != nil {
		return "", errParse
	}
	if parsed.Scheme == "" || parsed.Host == "" {
		return "", fmt.Errorf("invalid data-plane base URL %q", baseURL)
	}
	parsed.Path = strings.TrimRight(parsed.Path, "/") + "/v0/management/usage-queue"
	query := parsed.Query()
	query.Set("count", fmt.Sprintf("%d", count))
	parsed.RawQuery = query.Encode()
	return parsed.String(), nil
}

func dataPlaneUsageQueueAddress(baseURL string) (string, error) {
	parsed, errParse := url.Parse(strings.TrimSpace(baseURL))
	if errParse != nil {
		return "", errParse
	}
	if parsed.Scheme == "" || parsed.Host == "" {
		return "", fmt.Errorf("invalid data-plane base URL %q", baseURL)
	}
	return parsed.Host, nil
}

func isUsageControlPayload(payload []byte) bool {
	trimmed := strings.TrimSpace(string(payload))
	return trimmed == `{"support_refresh":true}` || trimmed == `{"refresh":true}`
}

func writeBridgeRESPCommand(conn net.Conn, args ...string) error {
	if conn == nil {
		return net.ErrClosed
	}
	if len(args) == 0 {
		return nil
	}
	var builder strings.Builder
	_, _ = fmt.Fprintf(&builder, "*%d\r\n", len(args))
	for _, arg := range args {
		_, _ = fmt.Fprintf(&builder, "$%d\r\n%s\r\n", len(arg), arg)
	}
	_, errWrite := io.WriteString(conn, builder.String())
	return errWrite
}

func readBridgeRESPLine(reader *bufio.Reader) (string, error) {
	line, errRead := reader.ReadString('\n')
	if errRead != nil {
		return "", errRead
	}
	if !strings.HasSuffix(line, "\r\n") {
		return "", fmt.Errorf("invalid RESP line terminator: %q", line)
	}
	return strings.TrimSuffix(line, "\r\n"), nil
}

func readBridgeRESPSimpleString(reader *bufio.Reader) (string, error) {
	prefix, errRead := reader.ReadByte()
	if errRead != nil {
		return "", errRead
	}
	if prefix == '-' {
		line, errLine := readBridgeRESPLine(reader)
		if errLine != nil {
			return "", errLine
		}
		return "", fmt.Errorf("%s", strings.TrimSpace(line))
	}
	if prefix != '+' {
		return "", fmt.Errorf("expected simple string prefix '+', got %q", prefix)
	}
	return readBridgeRESPLine(reader)
}

func readBridgeRESPBulkString(reader *bufio.Reader) ([]byte, error) {
	prefix, errRead := reader.ReadByte()
	if errRead != nil {
		return nil, errRead
	}
	if prefix != '$' {
		return nil, fmt.Errorf("expected bulk string prefix '$', got %q", prefix)
	}
	line, errLine := readBridgeRESPLine(reader)
	if errLine != nil {
		return nil, errLine
	}
	length, errParse := strconv.Atoi(line)
	if errParse != nil {
		return nil, fmt.Errorf("invalid bulk string length %q: %w", line, errParse)
	}
	if length < 0 {
		return nil, nil
	}
	payload := make([]byte, length+2)
	if _, errFull := io.ReadFull(reader, payload); errFull != nil {
		return nil, errFull
	}
	if payload[length] != '\r' || payload[length+1] != '\n' {
		return nil, fmt.Errorf("invalid bulk string terminator")
	}
	return payload[:length], nil
}

func readBridgeRESPArrayHeader(reader *bufio.Reader) (int, error) {
	prefix, errRead := reader.ReadByte()
	if errRead != nil {
		return 0, errRead
	}
	if prefix != '*' {
		return 0, fmt.Errorf("expected array prefix '*', got %q", prefix)
	}
	line, errLine := readBridgeRESPLine(reader)
	if errLine != nil {
		return 0, errLine
	}
	count, errParse := strconv.Atoi(line)
	if errParse != nil {
		return 0, fmt.Errorf("invalid array length %q: %w", line, errParse)
	}
	if count < 0 {
		return 0, fmt.Errorf("invalid array length %d", count)
	}
	return count, nil
}

func readBridgeRESPInteger(reader *bufio.Reader) (int, error) {
	prefix, errRead := reader.ReadByte()
	if errRead != nil {
		return 0, errRead
	}
	if prefix != ':' {
		return 0, fmt.Errorf("expected integer prefix ':', got %q", prefix)
	}
	line, errLine := readBridgeRESPLine(reader)
	if errLine != nil {
		return 0, errLine
	}
	value, errParse := strconv.Atoi(line)
	if errParse != nil {
		return 0, fmt.Errorf("invalid integer %q: %w", line, errParse)
	}
	return value, nil
}

func readBridgeRESPArrayOfBulkStrings(reader *bufio.Reader) ([][]byte, error) {
	count, errHeader := readBridgeRESPArrayHeader(reader)
	if errHeader != nil {
		return nil, errHeader
	}
	out := make([][]byte, 0, count)
	for i := 0; i < count; i++ {
		item, errItem := readBridgeRESPBulkString(reader)
		if errItem != nil {
			return nil, errItem
		}
		out = append(out, item)
	}
	return out, nil
}

func readBridgeRESPPubSubSubscribe(reader *bufio.Reader) (string, int, error) {
	count, errHeader := readBridgeRESPArrayHeader(reader)
	if errHeader != nil {
		return "", 0, errHeader
	}
	if count != 3 {
		return "", 0, fmt.Errorf("subscribe ack length = %d, want 3", count)
	}
	kind, errKind := readBridgeRESPBulkString(reader)
	if errKind != nil {
		return "", 0, errKind
	}
	if string(kind) != "subscribe" {
		return "", 0, fmt.Errorf("subscribe ack kind = %q", string(kind))
	}
	channel, errChannel := readBridgeRESPBulkString(reader)
	if errChannel != nil {
		return "", 0, errChannel
	}
	subscriptions, errSubscriptions := readBridgeRESPInteger(reader)
	if errSubscriptions != nil {
		return "", 0, errSubscriptions
	}
	return string(channel), subscriptions, nil
}

func readBridgeRESPPubSubMessage(reader *bufio.Reader) (string, []byte, error) {
	items, errItems := readBridgeRESPArrayOfBulkStrings(reader)
	if errItems != nil {
		return "", nil, errItems
	}
	if len(items) != 3 || string(items[0]) != "message" {
		return "", nil, fmt.Errorf("invalid pubsub message")
	}
	return string(items[1]), items[2], nil
}
