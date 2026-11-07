mkdir -p src/lib
echo 'TODO one' > src/a.txt
echo 'nothing' > src/b.txt
echo 'TODO two' > src/lib/c.txt
grep -rn TODO src | sort
grep -rl TODO src | sort
