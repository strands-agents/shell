echo hi | command grep -c hi
printf 'a\nb\n' | command sort -r
echo | command -v eval
command echo first | cat
echo hi | eval cat
echo 'echo sourced' > src.sh
echo | . ./src.sh
echo | source ./src.sh
echo hi | exec cat
echo | exit 3
echo "exit=$?"
f() { echo | return 4; echo "return=$?"; }
f
for i in 1 2; do echo | break; echo "loop $i"; done
set -- a b; echo | eval 'echo $1'
touch a.txt b.txt; echo | command echo '*'
(exit() { echo cleanup; builtin exit "$@"; }; echo | exit 3; echo "rc=$?")
