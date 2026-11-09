greet() {
  echo "args=$#: $1 $2"
  return 4
}
greet a b
echo "status=$?"
count() { local n=0; for _ in "$@"; do n=$((n + 1)); done; echo "$n"; }
count x y z
