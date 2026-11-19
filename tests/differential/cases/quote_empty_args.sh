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
show $empty""
unset missing
show $missing
show "$missing" x
show "$missing$@"
show "$@$missing"
show "$missing" "$@"
[ "$1" = "" ] && echo missing-positional-is-empty
[ "" = "$missing" ] && echo missing-variable-is-empty
show """" ""'' a""b
show "$@"
show "" "$@"
forward() { show "$@"; }
forward '' x
