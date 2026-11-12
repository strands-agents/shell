printf 'apple\nBanana\ncherry\napricot\n' > fruit.txt
grep ap fruit.txt
grep -n an fruit.txt
grep -c a fruit.txt
grep -i banana fruit.txt
grep -v a fruit.txt
grep -E '^(a|c)' fruit.txt
grep zzz fruit.txt
echo "nomatch=$?"
