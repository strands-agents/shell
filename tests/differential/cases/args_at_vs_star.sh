f() {
  for a in "$@"; do echo "at:[$a]"; done
  for a in "$*"; do echo "star:[$a]"; done
}
f 'x y' z
