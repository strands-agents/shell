e=
[ "$e" -lt 3 ]; echo "empty: $?"
[ abc -eq 0 ]; echo "word: $?"
test 3 -gt x; echo "right: $?"
[ 0x10 -eq 16 ]; echo "hex: $?"
[ 1.5 -ge 1 ]; echo "decimal: $?"
[ 99999999999999999999 -ne 1 ]; echo "overflow: $?"
[ 1 -eq 2 -a x -lt 2 ]; echo "after-and: $?"
[ ! "" -le 0 ]; echo "negated: $?"
if [ "$e" -lt 3 ]; then echo taken; else echo not-taken; fi
unset n
while [ "$n" -lt 2 ]; do echo "loop [$n]"; n=$((n+1)); done
echo end
