# wxdesk

Drives WeChat for Linux (the official client, 4.1) for a bot: it reads the
window through AT-SPI, the accessibility bus Qt exposes, and acts through
XTEST and the X clipboard. Nothing is hooked, injected or decrypted; it does
what a person at the screen would.

It keeps one group open and serves what it reads over TCP, one JSON value a
line (see `crates/wxdesk-proto`):

- messages, with their sender's display name, from the group and from
  private chats it opens when the session list shows news; images are
  copied from the window, files taken once WeChat has downloaded them;
- commands to send text with real @ mentions, images and files.

A person is known by their display name (a friend's by the remark you
give them), as the window shows no other id.

## Running it

`nixosModules.default` runs WeChat in an Xvnc display as the user `wx`,
with wxdesk beside it:

```nix
services.wxdesk = {
  enable = true;
  package = wxdesk.packages.x86_64-linux.wxdesk;
  wechat = wxdesk.packages.x86_64-linux.wechat;
};
```

Log WeChat in once through VNC on port 5901 (the password is in
`/var/lib/wxvnc/password`); the login lasts in `/home/wx`. Put the group's
name in `/var/lib/wxdesk/env` as `WXDESK_GROUP=…` (and the bot's display
name as `WXDESK_ME=…`). Clients send the content of `/var/lib/wxdesk/token`
first.

`nixosModules.vm` runs all that in a VM, `vm/guest.nix`, as the service
`wxdesk-vm`, which gives the guest the token and the env file as it boots:

```nix
services.wxdesk-vm = {
  enable = true;
  tokenFile = "/run/secrets/wxdesk/token";
  environmentFile = "/run/secrets/wxdesk/env";
};
```

The host reaches the guest's ssh, VNC and wxdesk on 127.0.0.1:10022, :15901
and :17710, and `wxdesk-vm-ssh` logs in as root with
`/var/lib/wxdesk-vm/ssh-key`, whose public half is in `vm/guest.nix`. A
deploy doesn't restart the VM, as booting may ask the phone to confirm the
login again: a new guest boots when `wxdesk-vm` is next started.
`scripts/try.sh` runs a build of wxdesk in the VM that runs, without
booting it.

## Limits

It reads only what the window shows: a chat that gets more messages between
two looks than fit on the screen loses the ones in between. Voice messages
come as their length; stickers, links and the like as WeChat names them.
