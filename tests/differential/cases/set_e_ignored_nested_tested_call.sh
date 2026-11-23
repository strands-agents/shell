set -e
inner() { false; echo in-inner; }
outer() { inner; echo in-outer; }
outer || echo caught
g() { false; echo in-g; }
true && g || echo caught-g
echo after
