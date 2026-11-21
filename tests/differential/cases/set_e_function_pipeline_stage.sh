set -e
f() { false; echo not-reached; }
f | cat
echo "rc=$?"
