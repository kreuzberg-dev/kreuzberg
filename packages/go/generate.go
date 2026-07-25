//go:generate go run ./cmd/setup -lib-dir .lib
//go:build ignore

// This file's sole purpose is to hold the go:generate directive that vendors the
// platform-specific native library into .lib/ for a writable checkout (e.g. this
// package's own source tree, before it is packaged for release). This is a
// dev-time convenience only — published consumers should invoke `cmd/setup`
// directly (see cmd/setup/main.go) rather than relying on `go generate`, since
// the Go module cache is read-only and `go generate` does not run for
// transitive dependencies. This file is not compiled but its directives are
// processed by `go generate`.
package main
