# Once a `+N` count is seen, a later -n keeps starting from a line number, as GNU tail does.
printf '1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n' > f.txt
tail -n +3 -n 11 f.txt
tail -n 2 -n +12 f.txt
tail -n +9 --lines=-11 f.txt
