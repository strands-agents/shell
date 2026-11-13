printf 'x' > 'back\slash'
printf 'y' > 'new
line'
md5sum 'back\slash' new*
md5sum 'back\slash' new* > sums
md5sum -c sums
