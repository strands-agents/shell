printf 'x' > 'back\slash'
printf 'y' > 'new
line'
sha256sum 'back\slash' new*
sha256sum 'back\slash' new* > sums
sha256sum -c sums
