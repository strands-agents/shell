# known-difference: fixed by #205
printf 'one\ntwo\n' | while read line; do echo "got $line"; done
