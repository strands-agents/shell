mkdir -p d/sub
echo x > d/f
ln -s d/f l1
ln -s l1 l2
ln -s d ld
ln -s missing dang
realpath --relative-to=. l2 ld/sub dang nofile ld/../l1
realpath --relative-to=d/sub l2 .
realpath -s --relative-to=. l2 ld/sub ld/../l1
realpath -m --relative-to=. no/x/../y d/f/x
realpath --relative-base=d d/f l2 /
realpath --relative-to=. no/x l1
echo "status $?"
realpath -q -e --relative-to=. dang
echo "status $?"
