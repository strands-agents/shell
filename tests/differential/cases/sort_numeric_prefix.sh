# known-difference: sort -n reads a key with text after the number ("1 b") as 0, so uniq -c | sort -rn doesn't sort
printf '10 x\n9 y\n100 z\n' | sort -n
