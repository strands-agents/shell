# A leading `-` on tail's -n count means the same as no sign (#253).
printf '1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n' > f.txt
tail -n -2 f.txt
tail -n -0 f.txt
tail --lines=-3 f.txt
