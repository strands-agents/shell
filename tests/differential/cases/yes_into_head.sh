yes | head -3
yes hello world | head -2
yes '' | head -2
yes -- -n | head -1
yes | head -c 5
echo
yes no | while read answer; do echo "got $answer"; break; done
