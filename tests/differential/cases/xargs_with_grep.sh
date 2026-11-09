mkdir src
echo needle > src/a.txt
echo hay > src/b.txt
find src -type f | sort | xargs grep -l needle
