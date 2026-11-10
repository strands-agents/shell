mkdir -p x/y
echo data > x/y/f.txt
cp x/y/f.txt x/copy.txt
mv x/copy.txt x/moved.txt
cat x/moved.txt
rm x/y/f.txt
[ -e x/y/f.txt ] || echo removed
rm -r x
[ -d x ] || echo dir-removed
