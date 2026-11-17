show() {
  printf 'argc=%s\n' "$#"
  for arg in "$@"; do printf '<%s>\n' "$arg"; done
}
show "" x
show "" ""
show '' x
empty=
show $empty
show "$empty" y
show """" ""'' a""b
show "$@"
show "" "$@"
forward() { show "$@"; }
forward '' x
