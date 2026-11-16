printf 'one\ntwo\nthree\n' > f
tac f
printf 'a\nb' | tac
printf 'x\ny\n' | tac f - f
printf 'a,b,c,' | tac -s ,
printf 'a\nb\n' | tac -b
tac missing f
echo "status $?"
