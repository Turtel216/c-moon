// Every function opens with the standard frame setup and closes by restoring
// the caller's frame, which `CHECK-NEXT` pins down line by line.
//@ compile-flags: --opt

// CHECK-LABEL: main:
// CHECK-NEXT: push rbp
// CHECK-NEXT: mov rbp, rsp

int main() {
    return 0;
}
