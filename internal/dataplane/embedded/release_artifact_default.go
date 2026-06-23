//go:build !release_embedded_artifact

package embedded

func bundledReleaseArtifact() *Artifact {
	return nil
}
