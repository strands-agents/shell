printf 'hello\n' > a
printf '\377\000\200' > bin
printf '' > empty
sha256sum a bin empty
printf 'hello' | sha256sum
printf 'hello' | sha256sum a - bin
sha256sum -b a
sha256sum --text bin
