# known-difference: bracket globs like [ab]1 and [!a]* don't expand
touch a1 a2 b1 ab
echo a?
echo [ab]1
echo [!a]*
