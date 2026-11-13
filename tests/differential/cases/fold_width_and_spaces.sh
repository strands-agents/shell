printf 'Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor\nshort\n\n' > f
fold -w 20 f
fold -s -w 20 f
fold -sw 12 f - < f
printf 'a\tbcdefghijkl\n' | fold -w 10
printf 'a\tbcdefghijkl\n' | fold -b -w 5
printf 'aa   bb  cc\n' | fold -s -w 4
printf 'abcdefgh' | fold -3
echo
fold -w 0 f
echo "status $?"
