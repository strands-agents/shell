printf 'false\necho reached\n' > child.sh
set -e
sh child.sh
echo "parent $?"
