set -e
f() { false; echo no; }
true && f
echo unreachable
