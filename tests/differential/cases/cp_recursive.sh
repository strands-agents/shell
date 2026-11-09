mkdir -p src/inner
echo a > src/inner/a.txt
cp -r src dst
cat dst/inner/a.txt
