# mkdir reports each directory it can't create and still creates the rest (#335).
mkdir nodir/a b
echo "rc=$?"
mkdir -p "" c
echo "rc=$?"
ls
