# known-difference: sed ignores a numeric flag like s/a/b/2 and replaces the first match
echo 'a/b/c' | sed 's|/|-|g'
echo 'aaa' | sed 's/a/b/2'
