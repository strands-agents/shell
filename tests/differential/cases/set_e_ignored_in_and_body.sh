set -e
f() { false; echo in-f; }
f && echo ok-f
{ false; echo in-g; } && echo ok-g
( false; echo in-s ) && echo ok-s
eval 'false; echo in-e' && echo ok-e
echo after
