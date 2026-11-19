grep() { echo wrapped; command grep "$@"; }
printf 'hi\nho\n' > f
grep -c hi f
echo "status=$?"
cat() { echo wrapped-cat; }
command cat f
unset -f cat
f() { echo fn-f; }
command f
echo "status=$?"
f
