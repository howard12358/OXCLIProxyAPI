package api

import (
	"net/http"
	"net/http/httputil"
	"net/url"
	"strings"

	"github.com/gin-gonic/gin"
	log "github.com/sirupsen/logrus"
)

func newDataPlaneResponsesReverseProxy(baseURL string) (*httputil.ReverseProxy, bool) {
	baseURL = strings.TrimSpace(baseURL)
	if baseURL == "" {
		return nil, false
	}

	target, err := url.Parse(baseURL)
	if err != nil {
		log.Errorf("invalid data plane responses base URL %q: %v", baseURL, err)
		return nil, false
	}

	proxy := httputil.NewSingleHostReverseProxy(target)
	originalDirector := proxy.Director
	proxy.Director = func(req *http.Request) {
		originalDirector(req)
		req.URL.Path = "/v1/responses"
		req.URL.RawPath = ""
		req.Host = target.Host
	}
	proxy.ErrorHandler = func(w http.ResponseWriter, req *http.Request, err error) {
		log.Errorf("data plane responses proxy error: %v", err)
		http.Error(w, "data plane unavailable", http.StatusBadGateway)
	}

	return proxy, true
}

func makeDataPlaneResponsesProxy(baseURL string) gin.HandlerFunc {
	proxy, ok := newDataPlaneResponsesReverseProxy(baseURL)
	if !ok {
		return nil
	}

	return func(c *gin.Context) {
		proxy.ServeHTTP(c.Writer, c.Request)
		c.Abort()
	}
}
