package com.strands.shell.wasm;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/** Mirrors tests/go/shell_test.go. */
class ShellWasmTest {

    private static void expect(ShellWasm.Result got, int status, String stdout) {
        assertEquals(status, got.status(), () -> "unexpected status, stderr: " + got.stderr());
        assertEquals(stdout, got.stdout(), () -> "unexpected stdout, stderr: " + got.stderr());
    }

    @Test
    void echo() {
        try (var h = new ShellWasm(null)) {
            int sh = h.mustNewShell("");
            expect(h.run(sh, "echo hello"), 0, "hello\n");
        }
    }

    @Test
    void pipeline() {
        try (var h = new ShellWasm(null)) {
            int sh = h.mustNewShell("");
            expect(h.run(sh, "printf 'b\\na\\n' | sort | head -n 1"), 0, "a\n");
        }
    }

    @Test
    void envPersists() {
        try (var h = new ShellWasm(null)) {
            int sh = h.mustNewShell("");
            expect(h.run(sh, "export X=1"), 0, "");
            expect(h.run(sh, "echo $X"), 0, "1\n");
        }
    }

    @Test
    void cwdPersists() {
        try (var h = new ShellWasm(null)) {
            int sh = h.mustNewShell("");
            expect(h.run(sh, "mkdir -p /tmp/d && cd /tmp/d"), 0, "");
            expect(h.run(sh, "pwd"), 0, "/tmp/d\n");
        }
    }

    @Test
    void filePersists() {
        try (var h = new ShellWasm(null)) {
            int sh = h.mustNewShell("");
            expect(h.run(sh, "echo hi > /tmp/f"), 0, "");
            expect(h.run(sh, "cat /tmp/f"), 0, "hi\n");
        }
    }

    @Test
    void copyBind(@TempDir Path hostDir) throws IOException {
        Files.writeString(hostDir.resolve("seed.txt"), "seed\n");
        try (var h = new ShellWasm(hostDir)) {
            int sh =
                    h.mustNewShell(
                            "[[bind]]\nsource = \"/host\"\ndestination = \"/work\"\nmode = \"copy\"\n");
            expect(h.run(sh, "cat /work/seed.txt"), 0, "seed\n");
            expect(h.run(sh, "echo x > /work/new && cat /work/new"), 0, "x\n");
            assertFalse(Files.exists(hostDir.resolve("new")), "copy bind leaked a write to the host");
        }
    }

    @Test
    void exitStatus() {
        try (var h = new ShellWasm(null)) {
            int sh = h.mustNewShell("");
            expect(h.run(sh, "exit 3"), 3, "");
        }
    }

    @Test
    void stderr() {
        try (var h = new ShellWasm(null)) {
            int sh = h.mustNewShell("");
            var got = h.run(sh, "echo oops >&2");
            assertEquals(0, got.status());
            assertEquals("", got.stdout());
            assertEquals("oops\n", got.stderr());
        }
    }

    @Test
    void luaArithmetic() {
        try (var h = new ShellWasm(null)) {
            int sh = h.mustNewShell("");
            expect(h.run(sh, "lua -e 'print(math.sqrt(144) == 12, 7 * 6)'"), 0, "true\t42\n");
        }
    }

    /** error() unwinds through Lua's setjmp/longjmp, i.e. wasm try_table/throw. */
    @Test
    void luaError() {
        try (var h = new ShellWasm(null)) {
            int sh = h.mustNewShell("");
            var got = h.run(sh, "lua -e 'error(\"boom\")'");
            assertNotEquals(0, got.status());
            assertTrue(got.stderr().contains("boom"), got.stderr());
            expect(h.run(sh, "lua -e 'print(1 + 1)'"), 0, "2\n");
        }
    }

    @Test
    void jq() {
        try (var h = new ShellWasm(null)) {
            int sh = h.mustNewShell("");
            expect(h.run(sh, "echo '{\"a\":1}' | jq .a"), 0, "1\n");
        }
    }

    @Test
    void curlUnsupported() {
        try (var h = new ShellWasm(null)) {
            int sh = h.mustNewShell("");
            var got = h.run(sh, "curl https://example.com");
            assertEquals(6, got.status(), got.stderr());
            assertTrue(got.stderr().contains("network access is not available"), got.stderr());
        }
    }

    @Test
    void independentShells() {
        try (var h = new ShellWasm(null)) {
            int a = h.mustNewShell("");
            int b = h.mustNewShell("");
            expect(h.run(a, "export X=a"), 0, "");
            expect(h.run(b, "echo \"[$X]\""), 0, "[]\n");
            expect(h.run(a, "echo \"[$X]\""), 0, "[a]\n");
        }
    }

    @Test
    void badConfig() {
        try (var h = new ShellWasm(null)) {
            assertEquals(0, h.newShell("nonsense = ["));
            assertFalse(h.lastError().isEmpty());
            assertEquals(0, h.newShell("[unknown]\nk = 1\n"));
            assertTrue(h.lastError().contains("unknown"), h.lastError());
        }
    }

    @Test
    void abiVersion() {
        try (var h = new ShellWasm(null)) {
            assertEquals(1, h.abiVersion());
        }
    }

    @Test
    void freeHandle() {
        try (var h = new ShellWasm(null)) {
            int sh = h.mustNewShell("");
            h.free(sh);
            assertEquals(0, h.rawRun(sh, "echo x"));
            assertEquals("invalid shell handle", h.lastError());
        }
    }
}
