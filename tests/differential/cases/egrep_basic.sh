printf 'apple\nBanana\ncherry\napricot\n' > fruit.txt
egrep '^(a|c)' fruit.txt
egrep -c 'an+a' fruit.txt
egrep -in 'BAN|err' fruit.txt
egrep -o 'p{2}|r+' fruit.txt
egrep zzz fruit.txt
echo "nomatch=$?"
