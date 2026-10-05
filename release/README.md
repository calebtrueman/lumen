# Release signing certificate

`lumen.cer` is the public certificate of the key that signs Lumen releases.
PCs that approved Lumen under Secure Boot trust exactly this certificate, so
every release must be signed with its private key (`keys/lumen.key`, kept
offline and never committed). `tools/dist.sh` refuses to build a release
signed with any other key.

SHA-256 fingerprint:
`36:37:FD:2F:1A:B0:9D:62:94:78:EC:89:EB:A2:A2:5F:E7:E8:3C:70:77:E2:02:97:E7:D3:79:11:D8:76:F3:41`

If the private key is ever lost, new releases need a new key, and every PC
must approve it once more. If it leaks, anything signed with it would boot
on PCs that trust it: remove it from each PC with
`mokutil --delete lumen.cer` and issue a new key.
