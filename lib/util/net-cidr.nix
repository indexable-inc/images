{ lib }:
let
  uintInRange =
    upper: value:
    value != ""
    && builtins.match "[0-9]+" value != null
    && (
      let
        n = lib.toInt value;
      in
      n >= 0 && n <= upper
    );

  isValidIpv4Cidr =
    s:
    let
      parts = lib.splitString "/" s;
      addrPart = if builtins.length parts == 2 then builtins.elemAt parts 0 else "";
      prefixPart = if builtins.length parts == 2 then builtins.elemAt parts 1 else "";
      octets = lib.splitString "." addrPart;
      octetIsByte = o: o != "" && builtins.match "[0-9]+" o != null && uintInRange 255 o;
    in
    builtins.isString s
    && builtins.length parts == 2
    && builtins.length octets == 4
    && builtins.all octetIsByte octets
    && uintInRange 32 prefixPart;

  isValidIpv6Cidr =
    s:
    let
      parts = lib.splitString "/" s;
      addrPart = if builtins.length parts == 2 then builtins.elemAt parts 0 else "";
      prefixPart = if builtins.length parts == 2 then builtins.elemAt parts 1 else "";
      hextets = lib.splitString ":" addrPart;
      nonEmptyHextets = builtins.filter (h: h != "") hextets;
      compressionParts = lib.splitString "::" addrPart;
      hasCompression = builtins.length compressionParts == 2;
      tooManyCompressionMarkers = builtins.length compressionParts > 2;
      hextetIsValid = h: builtins.match "[0-9A-Fa-f]{1,4}" h != null;
      hextetCount = builtins.length nonEmptyHextets;
    in
    builtins.isString s
    && builtins.length parts == 2
    && addrPart != ""
    && builtins.match ".*\\..*" addrPart == null
    && !(lib.hasInfix ":::" addrPart)
    && !tooManyCompressionMarkers
    && builtins.all hextetIsValid nonEmptyHextets
    && (if hasCompression then hextetCount < 8 else builtins.length hextets == 8 && hextetCount == 8)
    && uintInRange 128 prefixPart;

  isValidIpCidr = c: builtins.isString c && (isValidIpv4Cidr c || isValidIpv6Cidr c);
in
{
  inherit isValidIpv4Cidr isValidIpv6Cidr isValidIpCidr;
}
