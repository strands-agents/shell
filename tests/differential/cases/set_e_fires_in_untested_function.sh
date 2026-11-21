set -e
f() { false; echo no; }
f
echo unreachable
