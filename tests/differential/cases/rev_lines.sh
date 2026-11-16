printf 'abc\nhello world\n' > f
rev f
printf 'one\ntwo' | rev
printf 'a\tb\n\n' | rev
rev f missing f
echo "status $?"
