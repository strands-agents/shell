# known-difference: `local v` with no value keeps the outer value, where bash leaves v unset in the function
v=global
f() { local v; echo "v=[${v-unset}]"; }
f
export z=1
g() { local z; echo "z=[${z-unset}]"; }
g
echo "after g z=$z"
