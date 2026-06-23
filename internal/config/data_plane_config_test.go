package config

import "testing"

func TestDataPlaneConfigEffectiveMode(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name string
		cfg  DataPlaneConfig
		want string
	}{
		{
			name: "explicit embedded mode wins",
			cfg: DataPlaneConfig{
				Mode:             " embedded ",
				ResponsesBaseURL: "http://127.0.0.1:4100",
			},
			want: "embedded",
		},
		{
			name: "explicit external mode wins",
			cfg: DataPlaneConfig{
				Mode: "external",
				Embedded: EmbeddedDataPlaneConfig{
					Enabled: true,
				},
			},
			want: "external",
		},
		{
			name: "legacy responses base url implies external",
			cfg: DataPlaneConfig{
				ResponsesBaseURL: "http://127.0.0.1:4100",
			},
			want: "external",
		},
		{
			name: "embedded enabled implies embedded",
			cfg: DataPlaneConfig{
				Embedded: EmbeddedDataPlaneConfig{
					Enabled: true,
				},
			},
			want: "embedded",
		},
		{
			name: "empty config disables data plane",
			cfg:  DataPlaneConfig{},
			want: "",
		},
	}

	for _, tt := range tests {
		tt := tt
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()
			if got := tt.cfg.EffectiveMode(); got != tt.want {
				t.Fatalf("EffectiveMode() = %q, want %q", got, tt.want)
			}
		})
	}
}

func TestDataPlaneConfigEffectiveResponsesBaseURL(t *testing.T) {
	t.Parallel()

	cfg := DataPlaneConfig{
		ResponsesBaseURL:        "http://config.example",
		RuntimeResponsesBaseURL: " http://runtime.example ",
	}
	if got := cfg.EffectiveResponsesBaseURL(); got != "http://runtime.example" {
		t.Fatalf("EffectiveResponsesBaseURL() = %q, want %q", got, "http://runtime.example")
	}

	cfg.RuntimeResponsesBaseURL = ""
	if got := cfg.EffectiveResponsesBaseURL(); got != "http://config.example" {
		t.Fatalf("EffectiveResponsesBaseURL() = %q, want %q", got, "http://config.example")
	}
}
