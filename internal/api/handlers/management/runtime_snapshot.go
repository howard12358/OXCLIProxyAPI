package management

import (
	"net/http"
	"time"

	"github.com/gin-gonic/gin"
	dataplanesnapshot "github.com/router-for-me/CLIProxyAPI/v7/internal/dataplane/snapshot"
)

func (h *Handler) GetRuntimeSnapshot(c *gin.Context) {
	if h == nil || h.cfg == nil {
		c.JSON(http.StatusServiceUnavailable, gin.H{
			"error":   "runtime_snapshot_unavailable",
			"message": "management handler is not initialized",
		})
		return
	}

	h.mu.Lock()
	cfg := h.cfg.CloneForRuntime()
	authManager := h.authManager
	h.mu.Unlock()

	snapshot := dataplanesnapshot.BuildRuntimeSnapshot(cfg, authManager, time.Now().UTC())
	c.JSON(http.StatusOK, snapshot)
}
