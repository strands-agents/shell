mkdir -p d/tmp
echo hi > d/f
cd d
e=
rm -rf "$e"
echo "rm=$?"
touch "$e"
echo "touch=$?"
mkdir -p "$e"
echo "mkdir=$?"
mv "$e" x
echo "mv=$?"
ln -s "$e" l
echo "ln=$?"
[ -e "$e" ]
echo "test=$?"
cd "$e"
echo "cd=$?"
ls
