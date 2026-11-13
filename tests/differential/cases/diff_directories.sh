mkdir -p x/sub y/sub
echo s > x/same; echo s > y/same
echo old > x/f; echo new > y/f
echo o > x/onlyx; echo o > y/onlyy
echo a > x/sub/z; echo b > y/sub/z
diff x y; echo "status $?"
diff -r x y
diff -rq x y
diff -rN x y
