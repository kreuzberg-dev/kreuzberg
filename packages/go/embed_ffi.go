package xberg

import "embed"

// This file ensures that the C header is included when this module is vendored.
// The //go:embed directive tells Go to include the include/ directory when
// running `go mod vendor`. Native libraries are intentionally NOT embedded here
// — they are too large to commit and are downloaded at consume-time by
// `cmd/setup` into a per-user cache instead (see cmd/setup/main.go).
//
// Unlike the old vendor-only stub, this file is a real compiled member of the
// package: dropping the `//go:build ignore` guard is what makes `go mod vendor`
// (which only follows imports reachable from compiled files) include it.

//go:embed include/*
var _ embed.FS
