# WeChat for Linux: the AppImage Tencent serves now (Last-Modified
# 2026-11-20), not nixpkgs' archived 4.1.1.4. Its AppRun installs
# wechat-payload.tar into /opt/wechat-appimage with pkexec; here the payload
# is unpacked at build time and AppRun replaced by a plain launcher, then
# wrapped as nixpkgs' wechat/linux.nix does.
{ appimageTools, fetchurl }:
let
  pname = "wechat";
  version = "20261120";
  src = fetchurl {
    url = "https://dldir1v6.qq.com/weixin/Universal/Linux/WeChatLinux_x86_64.AppImage";
    hash = "sha256-ZbFSuutgutjU8bjlq/gAse/WHkKUUh1pQFgQFos/wsM=";
  };
  contents = appimageTools.extract {
    inherit pname version src;
    postExtract = ''
      mkdir -p $out/opt/wechat
      tar xf $out/wechat-payload.tar -C $out/opt/wechat
      rm $out/wechat-payload.tar $out/install-apparmor.sh
      if patchelf --print-needed $out/opt/wechat/wechat | grep -qx libtiff.so.5; then
        patchelf --replace-needed libtiff.so.5 libtiff.so $out/opt/wechat/wechat
      fi
      rm $out/AppRun
      cat > $out/AppRun <<'EOF'
      #!/bin/sh
      here=$(dirname "$(readlink -f "$0")")
      export APPDIR="$here/opt/wechat"
      export PATH="$APPDIR:$PATH"
      exec "$APPDIR/wechat" "$@"
      EOF
      chmod +x $out/AppRun
    '';
  };
in
appimageTools.wrapAppImage {
  inherit pname version;
  src = contents;
  meta.mainProgram = "wechat";
}
