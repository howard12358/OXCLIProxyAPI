package api

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/config"
	"github.com/router-for-me/CLIProxyAPI/v7/internal/redisqueue"
)

func TestDataPlaneUsageBridgePollsRustUsageQueueIntoCPAQueue(t *testing.T) {
	withDataPlaneUsageBridgeQueue(t, func() {
		calls := 0
		rs := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if r.URL.Path != "/v0/management/usage-queue" {
				t.Fatalf("path = %q, want /v0/management/usage-queue", r.URL.Path)
			}
			if got := r.URL.Query().Get("count"); got != "64" {
				t.Fatalf("count = %q, want 64", got)
			}
			calls++
			w.Header().Set("content-type", "application/json")
			if calls == 1 {
				_, _ = w.Write([]byte(`[{"request_id":"rs-1"},{"request_id":"rs-2"}]`))
				return
			}
			_, _ = w.Write([]byte(`[]`))
		}))
		t.Cleanup(rs.Close)

		ctx, cancel := context.WithCancel(context.Background())
		defer cancel()
		runDone := make(chan struct{})
		go func() {
			defer close(runDone)
			runDataPlaneUsageBridge(ctx, rs.URL, "")
		}()

		deadline := time.After(2 * time.Second)
		for {
			items := redisqueue.PopOldest(2)
			if len(items) == 2 {
				var first map[string]string
				if err := json.Unmarshal(items[0], &first); err != nil {
					t.Fatalf("unmarshal first item: %v", err)
				}
				if first["request_id"] != "rs-1" {
					t.Fatalf("first request_id = %q, want rs-1", first["request_id"])
				}
				cancel()
				<-runDone
				return
			}
			select {
			case <-deadline:
				t.Fatal("timed out waiting for bridged usage queue items")
			default:
				time.Sleep(10 * time.Millisecond)
			}
		}
	})
}

func TestDataPlaneUsageBridgeSubscribesRustUsageQueueIntoCPAQueue(t *testing.T) {
	withDataPlaneUsageBridgeQueue(t, func() {
		listener, errListen := net.Listen("tcp", "127.0.0.1:0")
		if errListen != nil {
			t.Fatalf("listen: %v", errListen)
		}
		t.Cleanup(func() { _ = listener.Close() })

		serverDone := make(chan struct{})
		go func() {
			defer close(serverDone)
			conn, errAccept := listener.Accept()
			if errAccept != nil {
				return
			}
			defer conn.Close()
			reader := bufio.NewReader(conn)
			args, errAuth := readBridgeTestRESPCommand(reader)
			if errAuth != nil || len(args) != 2 || strings.ToUpper(args[0]) != "AUTH" || args[1] != "rs-token" {
				t.Errorf("AUTH args = %#v err=%v", args, errAuth)
				return
			}
			_, _ = conn.Write([]byte("+OK\r\n"))
			args, errSubscribe := readBridgeTestRESPCommand(reader)
			if errSubscribe != nil || len(args) != 2 || strings.ToUpper(args[0]) != "SUBSCRIBE" || args[1] != "usage" {
				t.Errorf("SUBSCRIBE args = %#v err=%v", args, errSubscribe)
				return
			}
			_, _ = conn.Write([]byte("*3\r\n$9\r\nsubscribe\r\n$5\r\nusage\r\n:1\r\n"))
			_, _ = conn.Write([]byte("*3\r\n$7\r\nmessage\r\n$5\r\nusage\r\n$21\r\n{\"request_id\":\"rs-1\"}\r\n"))
		}()

		ctx, cancel := context.WithCancel(context.Background())
		defer cancel()
		runDone := make(chan struct{})
		go func() {
			defer close(runDone)
			runDataPlaneUsageBridge(ctx, "http://"+listener.Addr().String(), "rs-token")
		}()

		deadline := time.After(2 * time.Second)
		for {
			items := redisqueue.PopOldest(1)
			if len(items) == 1 {
				var first map[string]string
				if err := json.Unmarshal(items[0], &first); err != nil {
					t.Fatalf("unmarshal item: %v", err)
				}
				if first["request_id"] != "rs-1" {
					t.Fatalf("request_id = %q, want rs-1", first["request_id"])
				}
				cancel()
				<-runDone
				<-serverDone
				return
			}
			select {
			case <-deadline:
				t.Fatal("timed out waiting for subscribed usage queue item")
			default:
				time.Sleep(10 * time.Millisecond)
			}
		}
	})
}

func TestDataPlaneUsageBridgeBaseURLRequiresUsageAndQueue(t *testing.T) {
	cfg := &config.Config{
		DataPlane: config.DataPlaneConfig{ResponsesBaseURL: "http://127.0.0.1:4100"},
	}
	if got := dataPlaneUsageBridgeBaseURL(cfg); got != "" {
		t.Fatalf("baseURL without usage enabled = %q, want empty", got)
	}

	cfg.UsageStatisticsEnabled = true
	if got := dataPlaneUsageBridgeBaseURL(cfg); got != "" {
		t.Fatalf("baseURL without queue enabled = %q, want empty", got)
	}

	cfg.RemoteManagement.SecretKey = "management-key"
	if got := dataPlaneUsageBridgeBaseURL(cfg); got != "http://127.0.0.1:4100" {
		t.Fatalf("baseURL = %q, want data-plane URL", got)
	}
}

func withDataPlaneUsageBridgeQueue(t *testing.T, fn func()) {
	t.Helper()
	prevEnabled := redisqueue.Enabled()
	prevUsageEnabled := redisqueue.UsageStatisticsEnabled()
	redisqueue.SetEnabled(false)
	redisqueue.SetEnabled(true)
	redisqueue.SetUsageStatisticsEnabled(true)
	t.Cleanup(func() {
		redisqueue.SetEnabled(false)
		redisqueue.SetEnabled(prevEnabled)
		redisqueue.SetUsageStatisticsEnabled(prevUsageEnabled)
	})
	fn()
}

func readBridgeTestRESPCommand(reader *bufio.Reader) ([]string, error) {
	prefix, errRead := reader.ReadByte()
	if errRead != nil {
		return nil, errRead
	}
	if prefix != '*' {
		return nil, fmt.Errorf("expected RESP array prefix '*', got %q", prefix)
	}
	line, errLine := readTestRESPLine(reader)
	if errLine != nil {
		return nil, errLine
	}
	var count int
	if _, errScan := fmt.Sscanf(line, "%d", &count); errScan != nil {
		return nil, errScan
	}
	args := make([]string, 0, count)
	for i := 0; i < count; i++ {
		raw, errBulk := readTestRESPBulkString(reader)
		if errBulk != nil {
			return nil, errBulk
		}
		args = append(args, string(raw))
	}
	return args, nil
}
