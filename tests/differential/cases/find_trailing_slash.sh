mkdir -p dir/sub
touch dir/x dir/sub/x
find dir/ -name x | sort
find dir// -name x | sort
find dir/sub/ | sort
find ./dir/ -type f | sort
