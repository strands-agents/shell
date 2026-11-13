printf '1\n2\n3\n4\n5\n6\n7\n8\n9\n' > a
printf '1\nTWO\n3\n4\n5\n6\n8\n9\n10\n' > b
diff a b
echo "status $?"
# The unified header carries file times, so skip it.
diff -u a b | tail -n +3
diff -U1 a b | tail -n +3
diff -U0 a b | tail -n +3
