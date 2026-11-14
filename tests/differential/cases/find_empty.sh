mkdir -p one two empty
touch one/only two/a two/b
echo data > full
touch blank
find . -empty | sort
find one -empty
find one -type d -empty
find empty -empty
find . -type d -empty
find . -type f -empty | sort
find . -maxdepth 1 ! -empty | sort
