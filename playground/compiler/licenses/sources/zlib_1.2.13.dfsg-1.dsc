-----BEGIN PGP SIGNED MESSAGE-----
Hash: SHA512

Format: 3.0 (quilt)
Source: zlib
Binary: zlib1g, zlib1g-dev, zlib1g-udeb, lib64z1, lib64z1-dev, lib32z1, lib32z1-dev, libn32z1, libn32z1-dev
Architecture: any
Version: 1:1.2.13.dfsg-1
Maintainer: Mark Brown <broonie@debian.org>
Homepage: http://zlib.net/
Standards-Version: 4.6.1
Build-Depends: debhelper (>= 13), gcc-multilib [amd64 i386 kfreebsd-amd64 mips mipsel powerpc ppc64 s390 sparc s390x mipsn32 mipsn32el mipsr6 mipsr6el mipsn32r6 mipsn32r6el mips64 mips64el mips64r6 mips64r6el x32] <!nobiarch>, dpkg-dev (>= 1.16.1)
Package-List:
 lib32z1 deb libs optional arch=amd64,ppc64,kfreebsd-amd64,s390x profile=!nobiarch
 lib32z1-dev deb libdevel optional arch=amd64,ppc64,kfreebsd-amd64,s390x profile=!nobiarch
 lib64z1 deb libs optional arch=sparc,s390,i386,powerpc,mips,mipsel,mipsn32,mipsn32el,mipsr6,mipsr6el,mipsn32r6,mipsn32r6el,x32 profile=!nobiarch
 lib64z1-dev deb libdevel optional arch=sparc,s390,i386,powerpc,mips,mipsel,mipsn32,mipsn32el,mipsr6,mipsr6el,mipsn32r6,mipsn32r6el,x32 profile=!nobiarch
 libn32z1 deb libs optional arch=mips,mipsel profile=!nobiarch
 libn32z1-dev deb libdevel optional arch=mips,mipsel profile=!nobiarch
 zlib1g deb libs required arch=any
 zlib1g-dev deb libdevel optional arch=any
 zlib1g-udeb udeb debian-installer optional arch=any
Checksums-Sha1:
 4fec854a7c5fe543e44c12afa365642c0e6b5ded 1239825 zlib_1.2.13.dfsg.orig.tar.bz2
 41df4cb006cd857de65ec72f9538ffdf6d1d2f2c 15700 zlib_1.2.13.dfsg-1.debian.tar.xz
Checksums-Sha256:
 71feb7947e3c00ef125f83b79a4e529bde31171e5babe48b391f06758d1ab0a1 1239825 zlib_1.2.13.dfsg.orig.tar.bz2
 f66cf3d4f2d7defcd4d1fd1fb0a11ee39f1e01b42ec7d059c9dc5c1695133c44 15700 zlib_1.2.13.dfsg-1.debian.tar.xz
Files:
 ef268c6fda8701834db2d8322b0ccf4e 1239825 zlib_1.2.13.dfsg.orig.tar.bz2
 27c1389340e89c173dc30e625f119763 15700 zlib_1.2.13.dfsg-1.debian.tar.xz

-----BEGIN PGP SIGNATURE-----

iQFHBAEBCgAxFiEEreZoqmdXGLWf4p/qJNaLcl1Uh9AFAmNmdVsTHGJyb29uaWVA
ZGViaWFuLm9yZwAKCRAk1otyXVSH0EekB/9f+LM0uX2fQHhZa3dHD2EX59EwMUl6
qFCpgf03a4voYBtR059HTcBKNnaSlMJUsI3wiSxjcHSZXwFxYvxyriYX1kWN8Tr7
E32udKiq0xM9qfxcMT9V2bZQGCycgrKVNRXquJGBsk+tfD9qjdFmMWLeWYRavGEe
lceVPo5X/HyuR5F2rDLjbtzWQqmz5sF8TeFxjaeceU8FHWzbc2nJQSJhNr8xAPqx
ob4KuDxMsy9N6BHotSvSlMTN0QSOTmHRwdV8c0AuuCCkkF/Rnzbx3gf3Pyrt7BPq
Wq1tD940clQ7F7pH7lXofDKP8touc6EeIoXTYSVhbBlIwtNhHHKxZYta
=PY8v
-----END PGP SIGNATURE-----
