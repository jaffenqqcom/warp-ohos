# warp-ohos
使用说明：
1、运行fetch-full-code.sh脚本，下载完整的代码
2、需要在鸿蒙欧拉虚拟机里，运行warp/script/ohos/build-vm.sh编译代码（鸿蒙PC上，因为TLS限制在128，rust无法完成编译，所以要在欧拉虚拟机上编译）。但是欧拉虚拟机上有没有hap的编译和打包工具，所以编译和打包阶段，build-vm.sh又要ssh到鸿蒙系统上对hap编译和打包，鸿蒙系统需要运行sshd，并在build-vm.sh里修改用户名和密码。最好使用ssh密钥的方式（当前编译脚本里就是这样）
3、在warp/script/ohos 目录下，有下载编译工具的指导
4、在其他操作系统里，运行warp/script/ohos/build.sh，编译代码（未调试过）