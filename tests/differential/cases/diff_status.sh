echo one > a
echo two > b
printf 'x\0' > bin
diff a a; echo "same $?"
diff -q a b; echo "brief $?"
diff a bin; echo "binary $?"
echo one | diff - a; echo "stdin $?"
diff a missing; echo "missing $?"
diff --no-such-option a b; echo "bad option $?"
