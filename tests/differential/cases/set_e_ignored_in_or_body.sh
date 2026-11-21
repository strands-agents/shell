set -e
f() { false; echo in-f; }
f || echo caught-f
{ false; echo in-g; } || echo caught-g
( false; echo in-s ) || echo caught-s
eval 'false; echo in-e' || echo caught-e
echo after
