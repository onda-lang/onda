// Enum strings describe dense i32 ordinals; they never enter payload storage.
export function enumMemberCount(enumeration) {
  if (enumeration == null) return null;
  const members = enumeration.members;
  if (typeof enumeration.name !== "string" || !enumeration.name
    || !Array.isArray(members) || members.length === 0 || members.length > 0x7fffffff) {
    throw new TypeError("invalid enum metadata");
  }
  const names = new Set();
  for (const member of members) {
    if (typeof member?.name !== "string" || !member.name
      || typeof member.label !== "string" || names.has(member.name)) {
      throw new TypeError("invalid enum member metadata");
    }
    names.add(member.name);
  }
  return members.length;
}
