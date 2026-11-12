# compare: stdout, status, stderr-presence
rm no-such-file
echo "status=$?"
rm -f no-such-file
echo "force=$?"
