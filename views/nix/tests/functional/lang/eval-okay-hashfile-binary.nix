# The blob is not valid UTF-8 on purpose: hashFile digests the file's raw
# bytes (cppnix hashes what readFile would see, with no string in between),
# and an evaluator that repairs the encoding first computes a digest no
# other tool prints. ENG-13146.
map (a: builtins.hashFile a ./binary-blob.bin) [ "md5" "sha1" "sha256" "sha512" ]
