// wazero integration tests for the wasm32-wasip1 reactor (src/wasm_api.rs);
// mirrors tests/java. Build it first: ./scripts/build-wasm.sh --reactor --release
package shell_test

import (
	"context"
	"encoding/binary"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
	"github.com/tetratelabs/wazero/experimental"
	"github.com/tetratelabs/wazero/imports/wasi_snapshot_preview1"
)

const defaultWasm = "../../target/wasm32-wasip1/release/strands_shell.wasm"

var (
	ctx      = context.Background()
	runtime  wazero.Runtime
	compiled wazero.CompiledModule
)

func TestMain(m *testing.M) {
	path := os.Getenv("STRANDS_SHELL_WASM")
	if path == "" {
		path = defaultWasm
	}
	wasm, err := os.ReadFile(path)
	if err != nil {
		fmt.Fprintf(os.Stderr, "%s not found (build with ./scripts/build-wasm.sh --reactor --release)\n", path)
		if os.Getenv("CI") != "" {
			os.Exit(1) // a missing module must not pass silently in CI
		}
		os.Exit(0)
	}
	// Lua's setjmp/longjmp compiles to the exception-handling proposal.
	cfg := wazero.NewRuntimeConfig().
		WithCoreFeatures(api.CoreFeaturesV2 | experimental.CoreFeaturesExceptionHandling)
	runtime = wazero.NewRuntimeWithConfig(ctx, cfg)
	wasi_snapshot_preview1.MustInstantiate(ctx, runtime)
	compiled, err = runtime.CompileModule(ctx, wasm)
	if err != nil {
		fmt.Fprintf(os.Stderr, "compile: %v\n", err)
		os.Exit(1)
	}
	code := m.Run()
	runtime.Close(ctx)
	os.Exit(code)
}

type result struct {
	status int32
	stdout string
	stderr string
}

// harness is one reactor instance; hostDir, when set, is preopened at /host.
type harness struct {
	t   *testing.T
	mod api.Module
}

func newHarness(t *testing.T, hostDir string) *harness {
	cfg := wazero.NewModuleConfig().
		WithName("").
		WithStdout(os.Stdout).
		WithStderr(os.Stderr).
		WithStartFunctions("_initialize")
	if hostDir != "" {
		cfg = cfg.WithFSConfig(wazero.NewFSConfig().WithDirMount(hostDir, "/host"))
	}
	mod, err := runtime.InstantiateModule(ctx, compiled, cfg)
	if err != nil {
		t.Fatalf("instantiate: %v", err)
	}
	t.Cleanup(func() { mod.Close(ctx) })
	return &harness{t: t, mod: mod}
}

func (h *harness) call(name string, args ...uint64) uint64 {
	res, err := h.mod.ExportedFunction(name).Call(ctx, args...)
	if err != nil {
		h.t.Fatalf("%s: %v", name, err)
	}
	if len(res) == 0 {
		return 0
	}
	return res[0]
}

func (h *harness) write(s string) (ptr, n uint64) {
	n = uint64(len(s))
	ptr = h.call("shell_buf_alloc", n)
	if !h.mod.Memory().Write(uint32(ptr), []byte(s)) {
		h.t.Fatalf("memory write of %d bytes at %d failed", n, ptr)
	}
	return ptr, n
}

func (h *harness) read(packed uint64) string {
	ptr, n := uint32(packed>>32), uint32(packed)
	buf, ok := h.mod.Memory().Read(ptr, n)
	if !ok {
		h.t.Fatalf("memory read of %d bytes at %d failed", n, ptr)
	}
	return string(buf) // copies out of linear memory
}

func (h *harness) lastError() string {
	return h.read(h.call("shell_last_error"))
}

func (h *harness) newShell(toml string) uint64 {
	ptr, n := h.write(toml)
	defer h.call("shell_buf_free", ptr, n)
	return h.call("shell_new", ptr, n)
}

func (h *harness) mustNewShell(toml string) uint64 {
	handle := h.newShell(toml)
	if handle == 0 {
		h.t.Fatalf("shell_new: %s", h.lastError())
	}
	return handle
}

// rawRun returns the packed result pointer, or 0 for an invalid handle.
func (h *harness) rawRun(handle uint64, script string) uint64 {
	ptr, n := h.write(script)
	defer h.call("shell_buf_free", ptr, n)
	return h.call("shell_run", handle, ptr, n)
}

func (h *harness) run(handle uint64, script string) result {
	packed := h.rawRun(handle, script)
	if packed == 0 {
		h.t.Fatalf("shell_run: %s", h.lastError())
	}
	buf := []byte(h.read(packed))
	h.call("shell_buf_free", packed>>32, packed&0xffffffff)
	if len(buf) < 12 {
		h.t.Fatalf("result buffer too short: %d bytes", len(buf))
	}
	status := int32(binary.LittleEndian.Uint32(buf[0:4]))
	outLen := binary.LittleEndian.Uint32(buf[4:8])
	errLen := binary.LittleEndian.Uint32(buf[8:12])
	stdout := buf[12 : 12+outLen]
	stderr := buf[12+outLen : 12+outLen+errLen]
	return result{status, string(stdout), string(stderr)}
}

func expect(t *testing.T, got result, status int32, stdout string) {
	t.Helper()
	if got.status != status || got.stdout != stdout {
		t.Fatalf("got status=%d stdout=%q stderr=%q; want status=%d stdout=%q",
			got.status, got.stdout, got.stderr, status, stdout)
	}
}

func TestEcho(t *testing.T) {
	h := newHarness(t, "")
	sh := h.mustNewShell("")
	expect(t, h.run(sh, "echo hello"), 0, "hello\n")
}

func TestPipeline(t *testing.T) {
	h := newHarness(t, "")
	sh := h.mustNewShell("")
	expect(t, h.run(sh, "printf 'b\\na\\n' | sort | head -n 1"), 0, "a\n")
}

func TestEnvPersists(t *testing.T) {
	h := newHarness(t, "")
	sh := h.mustNewShell("")
	expect(t, h.run(sh, "export X=1"), 0, "")
	expect(t, h.run(sh, "echo $X"), 0, "1\n")
}

func TestCwdPersists(t *testing.T) {
	h := newHarness(t, "")
	sh := h.mustNewShell("")
	expect(t, h.run(sh, "mkdir -p /tmp/d && cd /tmp/d"), 0, "")
	expect(t, h.run(sh, "pwd"), 0, "/tmp/d\n")
}

func TestFilePersists(t *testing.T) {
	h := newHarness(t, "")
	sh := h.mustNewShell("")
	expect(t, h.run(sh, "echo hi > /tmp/f"), 0, "")
	expect(t, h.run(sh, "cat /tmp/f"), 0, "hi\n")
}

func TestCopyBind(t *testing.T) {
	hostDir := t.TempDir()
	if err := os.WriteFile(filepath.Join(hostDir, "seed.txt"), []byte("seed\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	h := newHarness(t, hostDir)
	sh := h.mustNewShell("[[bind]]\nsource = \"/host\"\ndestination = \"/work\"\nmode = \"copy\"\n")
	expect(t, h.run(sh, "cat /work/seed.txt"), 0, "seed\n")
	expect(t, h.run(sh, "echo x > /work/new && cat /work/new"), 0, "x\n")
	if _, err := os.Stat(filepath.Join(hostDir, "new")); !os.IsNotExist(err) {
		t.Fatalf("copy bind leaked a write to the host: %v", err)
	}
}

func TestExitStatus(t *testing.T) {
	h := newHarness(t, "")
	sh := h.mustNewShell("")
	expect(t, h.run(sh, "exit 3"), 3, "")
}

func TestStderr(t *testing.T) {
	h := newHarness(t, "")
	sh := h.mustNewShell("")
	got := h.run(sh, "echo oops >&2")
	if got.status != 0 || got.stdout != "" || got.stderr != "oops\n" {
		t.Fatalf("got %+v", got)
	}
}

func TestLuaArithmetic(t *testing.T) {
	h := newHarness(t, "")
	sh := h.mustNewShell("")
	expect(t, h.run(sh, "lua -e 'print(math.sqrt(144) == 12, 7 * 6)'"), 0, "true\t42\n")
}

// error() unwinds through Lua's setjmp/longjmp, i.e. wasm try_table/throw.
func TestLuaError(t *testing.T) {
	h := newHarness(t, "")
	sh := h.mustNewShell("")
	got := h.run(sh, "lua -e 'error(\"boom\")'")
	if got.status == 0 || !strings.Contains(got.stderr, "boom") {
		t.Fatalf("got %+v", got)
	}
	// The VM is still usable afterwards.
	expect(t, h.run(sh, "lua -e 'print(1 + 1)'"), 0, "2\n")
}

func TestJq(t *testing.T) {
	h := newHarness(t, "")
	sh := h.mustNewShell("")
	expect(t, h.run(sh, "echo '{\"a\":1}' | jq .a"), 0, "1\n")
}

func TestCurlUnsupported(t *testing.T) {
	h := newHarness(t, "")
	sh := h.mustNewShell("")
	got := h.run(sh, "curl https://example.com")
	if got.status != 6 || !strings.Contains(got.stderr, "network access is not available") {
		t.Fatalf("got %+v", got)
	}
}

func TestIndependentShells(t *testing.T) {
	h := newHarness(t, "")
	a := h.mustNewShell("")
	b := h.mustNewShell("")
	expect(t, h.run(a, "export X=a"), 0, "")
	expect(t, h.run(b, "echo \"[$X]\""), 0, "[]\n")
	expect(t, h.run(a, "echo \"[$X]\""), 0, "[a]\n")
}

func TestBadConfig(t *testing.T) {
	h := newHarness(t, "")
	if got := h.newShell("nonsense = ["); got != 0 {
		t.Fatalf("invalid TOML accepted: handle %d", got)
	}
	if h.lastError() == "" {
		t.Fatal("no error message for invalid TOML")
	}
	if got := h.newShell("[unknown]\nk = 1\n"); got != 0 {
		t.Fatalf("unknown table accepted: handle %d", got)
	}
	if msg := h.lastError(); !strings.Contains(msg, "unknown") {
		t.Fatalf("unexpected error message: %q", msg)
	}
}

func TestFreeHandle(t *testing.T) {
	h := newHarness(t, "")
	sh := h.mustNewShell("")
	h.call("shell_free", sh)
	if packed := h.rawRun(sh, "echo x"); packed != 0 {
		t.Fatalf("run on a freed handle returned %#x", packed)
	}
	if msg := h.lastError(); msg != "invalid shell handle" {
		t.Fatalf("unexpected error message: %q", msg)
	}
}
