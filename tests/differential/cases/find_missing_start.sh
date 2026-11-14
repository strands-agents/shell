# known-difference: a missing start path is skipped silently with status 0, where find reports it and exits 1
mkdir -p dir
touch dir/x
find missing_dir; echo "status $?"
find missing_dir dir > found; echo "status $?"
sort found
