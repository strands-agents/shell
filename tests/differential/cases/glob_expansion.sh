touch b.txt a.txt c.log
echo *.txt
echo *.none
for f in *.log; do echo "log: $f"; done
