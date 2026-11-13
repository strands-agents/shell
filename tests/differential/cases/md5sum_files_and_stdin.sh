printf 'hello\n' > a
printf '\377\000\200' > bin
printf '' > empty
md5sum a bin empty
printf 'hello' | md5sum
printf 'hello' | md5sum a - bin
md5sum -b a
md5sum --text bin
