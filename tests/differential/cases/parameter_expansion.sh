path=dir/sub/file.tar.gz
echo "${path##*/}"
echo "${path%/*}"
echo "${path%.*}"
echo "${path%%.*}"
echo "${#path}"
echo "${unset_var:-default}"
v=set
echo "${v:+alt}"
