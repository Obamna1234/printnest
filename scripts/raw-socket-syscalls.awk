# Finds raw Linux x86_64 socket syscalls in `objdump -d --no-show-raw-insn`
# output, the way crates like rustix make them (no libc import to see).
#
# Prints "<function>\t<syscall name>" for every hit:
#  - a `syscall` instruction whose %eax was just loaded with a socket
#    syscall number, or
#  - a call to libc's syscall() whose first argument (%edi) was just loaded
#    with one.
#
# Socket-creating / connecting / serving syscalls only (x86_64 numbers):
#   41 socket, 42 connect, 43 accept, 49 bind, 50 listen, 288 accept4

BEGIN {
    names["0x29"] = "socket";  names["0x2a"] = "connect"; names["0x2b"] = "accept"
    names["0x31"] = "bind";    names["0x32"] = "listen";  names["0x120"] = "accept4"
    window = 12
}

/^[0-9a-f]+ </ { fn = $2; eax = ""; edi = ""; eax_age = 99; edi_age = 99; next }

{
    eax_age++; edi_age++
    if (match($0, /mov +\$0x[0-9a-f]+,%(eax|rax)/)) {
        v = substr($0, RSTART, RLENGTH); sub(/^mov +\$/, "", v); sub(/,.*/, "", v)
        eax = v; eax_age = 0
    }
    if (match($0, /mov +\$0x[0-9a-f]+,%(edi|rdi)/)) {
        v = substr($0, RSTART, RLENGTH); sub(/^mov +\$/, "", v); sub(/,.*/, "", v)
        edi = v; edi_age = 0
    }
    if ($0 ~ /[ \t]syscall[ \t]*$/ && eax_age <= window && (eax in names)) {
        print fn "\t" names[eax]
    }
    if ($0 ~ /(call|jmp).*<syscall@/ && edi_age <= window && (edi in names)) {
        print fn "\t" names[edi]
    }
}
