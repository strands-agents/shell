printf 'foo bar\nfoo baz\n' > f.txt
sed 's/foo/qux/' f.txt
sed -n '2p' f.txt
sed -E 's/(b..)/[\1]/g' f.txt
sed -i 's/bar/BAR/' f.txt
cat f.txt
