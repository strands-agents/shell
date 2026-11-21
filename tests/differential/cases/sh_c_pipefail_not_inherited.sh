set -o pipefail
sh -c 'false | true; echo "child $?"'
