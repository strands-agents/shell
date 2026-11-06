out=$(ls does-not-exist 2>&1)
[ -n "$out" ] && echo captured
ls does-not-exist 2>&1 | grep -q . && echo piped
