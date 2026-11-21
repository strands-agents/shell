# A prefix assignment on `local` must not leak into the caller's variable (#308).
x=g
f() { x=1 local x=2; echo "in:$x"; }
f
echo "after:$x"
