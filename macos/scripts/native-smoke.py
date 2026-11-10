"""Launch the packaged app and require a native/frontend handshake."""
import subprocess, sys
result = subprocess.run([sys.argv[1], "--yas-smoke-test"], capture_output=True, text=True, timeout=45)
print(result.stdout)
print(result.stderr)
if result.returncode or "YAS_NATIVE_SMOKE_OK" not in result.stdout:
    raise SystemExit("Native application smoke test failed")
