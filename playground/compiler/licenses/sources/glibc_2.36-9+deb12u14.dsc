-----BEGIN PGP SIGNED MESSAGE-----
Hash: SHA512

Format: 3.0 (quilt)
Source: glibc
Binary: libc-bin, libc-dev-bin, libc-devtools, libc-l10n, glibc-doc, glibc-source, locales, locales-all, nscd, libc6, libc6-dev, libc6-dbg, libc6-udeb, libc6.1, libc6.1-dev, libc6.1-dbg, libc6.1-udeb, libc0.3, libc0.3-dev, libc0.3-dbg, libc0.3-udeb, libc0.1, libc0.1-dev, libc0.1-dbg, libc0.1-udeb, libc6-i386, libc6-dev-i386, libc6-sparc, libc6-dev-sparc, libc6-sparc64, libc6-dev-sparc64, libc6-s390, libc6-dev-s390, libc6-amd64, libc6-dev-amd64, libc6-powerpc, libc6-dev-powerpc, libc6-ppc64, libc6-dev-ppc64, libc6-mips32, libc6-dev-mips32, libc6-mipsn32, libc6-dev-mipsn32, libc6-mips64, libc6-dev-mips64, libc0.1-i386, libc0.1-dev-i386, libc6-x32, libc6-dev-x32, libc6.1-alphaev67
Architecture: any all
Version: 2.36-9+deb12u14
Maintainer: GNU Libc Maintainers <debian-glibc@lists.debian.org>
Uploaders: Clint Adams <clint@debian.org>, Aurelien Jarno <aurel32@debian.org>, Samuel Thibault <sthibault@debian.org>
Homepage: https://www.gnu.org/software/libc/libc.html
Standards-Version: 4.6.2
Vcs-Browser: https://salsa.debian.org/glibc-team/glibc
Vcs-Git: https://salsa.debian.org/glibc-team/glibc.git
Testsuite: autopkgtest
Testsuite-Triggers: @builddeps@, binutils, fakeroot, gcc-12, linux-libc-dev
Build-Depends: gettext, dpkg (>= 1.18.7), dpkg-dev (>= 1.17.14), xz-utils, file, quilt, autoconf, gawk, debhelper-compat (= 13), rdfind, symlinks, netbase, gperf, bison, linux-libc-dev (>= 3.9) [linux-any], libaudit-dev [linux-any], libcap-dev [linux-any] <!stage2>, libselinux1-dev [linux-any] <!stage2>, mig-for-host (>= 1.8+git20200618-7~) [hurd-i386], gnumach-dev (>= 2:1.8+git20200710-2~) [hurd-i386], hurd-dev (>= 1:0.9.git20201127-4~) [hurd-i386] | hurd-headers-dev [hurd-i386], kfreebsd-kernel-headers [kfreebsd-any], binutils-for-host (>= 2.38), g++-12 <!cross>, g++-12-multilib [amd64 i386 kfreebsd-amd64 mips mipsel mipsn32 mipsn32el mips64 mips64el mipsr6 mipsr6el mipsn32r6 mipsn32r6el mips64r6 mips64r6el powerpc ppc64 s390x sparc sparc64 x32] <!nobiarch>, g++-12-x86-64-kfreebsd-gnu [kfreebsd-amd64] <cross>, g++-12-i686-kfreebsd-gnu [kfreebsd-i386] <cross>, g++-12-x86-64-linux-gnu [amd64] <cross>, g++-12-arc-linux-gnu [arc] <cross>, g++-12-aarch64-linux-gnu [arm64] <cross>, g++-12-arm-linux-gnueabi [armel] <cross>, g++-12-arm-linux-gnueabihf [armhf] <cross>, g++-12-hppa-linux-gnu [hppa] <cross>, g++-12-i686-linux-gnu [i386] <cross>, g++-12-m68k-linux-gnu [m68k] <cross>, g++-12-mips-linux-gnu [mips] <cross>, g++-12-mipsel-linux-gnu [mipsel] <cross>, g++-12-mips64-linux-gnuabin32 [mipsn32] <cross>, g++-12-mips64el-linux-gnuabin32 [mipsn32el] <cross>, g++-12-mips64-linux-gnuabi64 [mips64] <cross>, g++-12-mips64el-linux-gnuabi64 [mips64el] <cross>, g++-12-mipsisa32r6-linux-gnu [mipsr6] <cross>, g++-12-mipsisa32r6el-linux-gnu [mipsr6el] <cross>, g++-12-mipsisa64r6-linux-gnuabin32 [mipsn32r6] <cross>, g++-12-mipsisa64r6el-linux-gnuabin32 [mipsn32r6el] <cross>, g++-12-mipsisa64r6-linux-gnuabi64 [mips64r6] <cross>, g++-12-mipsisa64r6el-linux-gnuabi64 [mips64r6el] <cross>, g++-12-nios2-linux-gnu [nios2] <cross>, g++-12-powerpc-linux-gnu [powerpc] <cross>, g++-12-powerpc64-linux-gnu [ppc64] <cross>, g++-12-powerpc64le-linux-gnu [ppc64el] <cross>, g++-12-riscv64-linux-gnu [riscv64] <cross>, g++-12-sparc-linux-gnu [sparc] <cross>, g++-12-sparc64-linux-gnu [sparc64] <cross>, g++-12-s390x-linux-gnu [s390x] <cross>, g++-12-sh3-linux-gnu [sh3] <cross>, g++-12-sh4-linux-gnu [sh4] <cross>, g++-12-x86-64-linux-gnux32 [x32] <cross>, g++-12-alpha-linux-gnu [alpha] <cross>, g++-12-ia64-linux-gnu [ia64] <cross>, python3:native, libidn2-0 (>= 2.0.5~) <!nocheck>, libc-bin (>= 2.36) <cross>, libgd-dev <!stage1 !stage2>
Build-Depends-Indep: perl, po-debconf (>= 1.0)
Package-List:
 glibc-doc deb doc optional arch=all profile=!stage1
 glibc-source deb devel optional arch=all profile=!stage1
 libc-bin deb libs required arch=any profile=!stage1 essential=yes
 libc-dev-bin deb libdevel optional arch=any profile=!stage1
 libc-devtools deb devel optional arch=any profile=!stage1+!stage2
 libc-l10n deb localization standard arch=all profile=!stage1
 libc0.1 deb libs optional arch=kfreebsd-amd64,kfreebsd-i386 profile=!stage1
 libc0.1-dbg deb debug optional arch=kfreebsd-amd64,kfreebsd-i386 profile=!stage1
 libc0.1-dev deb libdevel optional arch=kfreebsd-amd64,kfreebsd-i386
 libc0.1-dev-i386 deb libdevel optional arch=kfreebsd-amd64 profile=!nobiarch
 libc0.1-i386 deb libs optional arch=kfreebsd-amd64 profile=!stage1,!nobiarch
 libc0.1-udeb udeb debian-installer optional arch=kfreebsd-amd64,kfreebsd-i386 profile=!noudeb,!stage1
 libc0.3 deb libs optional arch=hurd-i386 profile=!stage1
 libc0.3-dbg deb debug optional arch=hurd-i386 profile=!stage1
 libc0.3-dev deb libdevel optional arch=hurd-i386
 libc0.3-udeb udeb debian-installer optional arch=hurd-i386 profile=!noudeb,!stage1
 libc6 deb libs optional arch=amd64,arc,arm64,armel,armhf,hppa,i386,m68k,mips,mipsel,mipsn32,mipsn32el,mips64,mips64el,mipsr6,mipsr6el,mipsn32r6,mipsn32r6el,mips64r6,mips64r6el,nios2,powerpc,ppc64,ppc64el,riscv64,sparc,sparc64,s390x,sh3,sh4,x32 profile=!stage1
 libc6-amd64 deb libs optional arch=i386,x32 profile=!stage1,!nobiarch
 libc6-dbg deb debug optional arch=amd64,arc,arm64,armel,armhf,hppa,i386,m68k,mips,mipsel,mipsn32,mipsn32el,mips64,mips64el,mipsr6,mipsr6el,mipsn32r6,mipsn32r6el,mips64r6,mips64r6el,nios2,powerpc,ppc64,ppc64el,riscv64,sparc,sparc64,s390x,sh3,sh4,x32 profile=!stage1
 libc6-dev deb libdevel optional arch=amd64,arc,arm64,armel,armhf,hppa,i386,m68k,mips,mipsel,mipsn32,mipsn32el,mips64,mips64el,mipsr6,mipsr6el,mipsn32r6,mipsn32r6el,mips64r6,mips64r6el,nios2,powerpc,ppc64,ppc64el,riscv64,sparc,sparc64,s390x,sh3,sh4,x32
 libc6-dev-amd64 deb libdevel optional arch=i386,x32 profile=!nobiarch
 libc6-dev-i386 deb libdevel optional arch=amd64,x32 profile=!nobiarch
 libc6-dev-mips32 deb libdevel optional arch=mipsn32,mipsn32el,mips64,mips64el,mipsn32r6,mipsn32r6el,mips64r6,mips64r6el profile=!nobiarch
 libc6-dev-mips64 deb libdevel optional arch=mips,mipsel,mipsn32,mipsn32el,mipsr6,mipsr6el,mipsn32r6,mipsn32r6el profile=!nobiarch
 libc6-dev-mipsn32 deb libdevel optional arch=mips,mipsel,mips64,mips64el,mipsr6,mipsr6el,mips64r6,mips64r6el profile=!nobiarch
 libc6-dev-powerpc deb libdevel optional arch=ppc64 profile=!nobiarch
 libc6-dev-ppc64 deb libdevel optional arch=powerpc profile=!nobiarch
 libc6-dev-s390 deb libdevel optional arch=s390x profile=!nobiarch
 libc6-dev-sparc deb libdevel optional arch=sparc64 profile=!nobiarch
 libc6-dev-sparc64 deb libdevel optional arch=sparc profile=!nobiarch
 libc6-dev-x32 deb libdevel optional arch=amd64,i386 profile=!nobiarch
 libc6-i386 deb libs optional arch=amd64,x32 profile=!stage1,!nobiarch
 libc6-mips32 deb libs optional arch=mipsn32,mipsn32el,mips64,mips64el,mipsn32r6,mipsn32r6el,mips64r6,mips64r6el profile=!stage1,!nobiarch
 libc6-mips64 deb libs optional arch=mips,mipsel,mipsn32,mipsn32el,mipsr6,mipsr6el,mipsn32r6,mipsn32r6el profile=!stage1,!nobiarch
 libc6-mipsn32 deb libs optional arch=mips,mipsel,mips64,mips64el,mipsr6,mipsr6el,mips64r6,mips64r6el profile=!stage1,!nobiarch
 libc6-powerpc deb libs optional arch=ppc64 profile=!stage1,!nobiarch
 libc6-ppc64 deb libs optional arch=powerpc profile=!stage1,!nobiarch
 libc6-s390 deb libs optional arch=s390x profile=!stage1,!nobiarch
 libc6-sparc deb libs optional arch=sparc64 profile=!stage1,!nobiarch
 libc6-sparc64 deb libs optional arch=sparc profile=!stage1,!nobiarch
 libc6-udeb udeb debian-installer optional arch=amd64,arc,arm64,armel,armhf,hppa,i386,m68k,mips,mipsel,mipsn32,mipsn32el,mips64,mips64el,mipsr6,mipsr6el,mipsn32r6,mipsn32r6el,mips64r6,mips64r6el,nios2,powerpc,ppc64,ppc64el,riscv64,sparc,sparc64,s390x,sh3,sh4,x32 profile=!noudeb,!stage1
 libc6-x32 deb libs optional arch=amd64,i386 profile=!stage1,!nobiarch
 libc6.1 deb libs optional arch=alpha,ia64 profile=!stage1
 libc6.1-alphaev67 deb libs optional arch=alpha profile=!stage1
 libc6.1-dbg deb debug optional arch=alpha,ia64 profile=!stage1
 libc6.1-dev deb libdevel optional arch=alpha,ia64
 libc6.1-udeb udeb debian-installer optional arch=alpha,ia64 profile=!noudeb,!stage1
 locales deb localization standard arch=all profile=!stage1
 locales-all deb localization optional arch=any profile=!stage1
 nscd deb admin optional arch=any profile=!stage1
Checksums-Sha1:
 d034128a6116dcf9e76c92f898ec63f4f4e65786 19363988 glibc_2.36.orig.tar.xz
 42404623ac3ac7cb1bcce7dc7441ef3782c13871 918488 glibc_2.36-9+deb12u14.debian.tar.xz
Checksums-Sha256:
 a543c02070d46ccaf866957efd13f10c924daa74c86a90a0254db09a92a708ee 19363988 glibc_2.36.orig.tar.xz
 cf4ac9cd98185452cae3ef34e2e4ee12753e3d93fd0c62c61396d4a47eec902f 918488 glibc_2.36-9+deb12u14.debian.tar.xz
Files:
 b2b0246bc50781a2243f8141feeb1bf4 19363988 glibc_2.36.orig.tar.xz
 caad7ed8eedd10944370b3d01d08e3dd 918488 glibc_2.36-9+deb12u14.debian.tar.xz

-----BEGIN PGP SIGNATURE-----

iQIzBAEBCgAdFiEEUryGlb40+QrX1Ay4E4jA+JnoM2sFAmn3cG8ACgkQE4jA+Jno
M2s4zw//fZGRw6DktGYyQOM5blZ47ts5lr9+jFliSOlMU6AJnJ3cirGzC00/hhJt
VnTgjjR8dfFYzUqtJZIvHbsIRcXpP1yN1xJNfx4EiKzhik39+mdaLvQm9n2dyRxk
TKKtPlI5E8w99FUbTO7d1Jmd8iY02NQrxGN8SfmS3tZI0GCZHVJpW7z5kFIr/p6Q
TsBnTPepso+yO+3ICpcgPF0IPHcpF8r+d0xGqWV+LxaczlAcHlow5rjxqj7BtlFm
XzfgQABCWNuD/c+inKgGQcoGSHH7zdHds00Awy+bIbsZzlNHiePlFWKpxR7AmdCX
m/BRlKLYhbVj2u2kmQbRmHfmdXNB98cxnJXcR62jOPapi4MJJxuBAf8iXOJLXK1x
MT2bIiw3kR/vBUWUEgveSpzswDfndXU5GN6V9SEnrZIE1GfL5znh9WC+RRzDx1xp
2eB6WtVT1hTaSSRzn7Aw4SO7hRAaoPWFsC0boBkYreO5SL6EYsXCQbNk2ZzJT7md
T+dAbBUsSItyTK8t6tcgVigIQxwJtOcUA/QWWamDlzYZLjs8DXqtz18AjGwA/pjf
9i0Ytu+NsVws92ENUYLEK65EvuOfPH1X0kZVd8nDWcjk1FWtf/PCPEPlA5ln+bEp
feivrMerq6XyN4+gzdM/maRYkFbenp5BjQwF+OXCtVEOUap24co=
=ovsp
-----END PGP SIGNATURE-----
