touch -d 2001-02-03T04:05:06Z old
touch -d 2002-02-03T04:05:06Z new
[ new -nt old ] && echo "-d sets the time"
touch -r new old
[ new -nt old ] || echo "-r copies the time"
touch -m -t 200001010000 old
[ old -ot new ] && echo "-t sets the time"
touch -a -d 1990-01-01 new
[ new -nt old ] && echo "-a keeps the modification time"
touch -c missing
[ -e missing ] || echo "-c creates nothing"
echo kept > content
touch -d @981173106 content
cat content
touch -d 2001-02-03 fresh
[ -f fresh ] && [ fresh -ot new ] && echo "a new file takes the date"
