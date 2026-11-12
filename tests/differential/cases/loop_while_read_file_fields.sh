printf 'alice 30\nbob 25\n' > people.txt
while read name age; do echo "$name is $age"; done < people.txt
