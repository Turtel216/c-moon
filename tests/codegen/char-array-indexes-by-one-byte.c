// Elements of a `char[]` are one byte apart, so a computed index is scaled by
// one in the addressing mode and the access touches a single byte -- where an
// `int` array would have scaled by four.

// CHECK-LABEL: main:
// CHECK: BYTE PTR [rbp +
// CHECK-NOT: *4
// CHECK-NOT: *8

int main() {
    char letters[4];
    int i = 0;
    while (i < 4) {
        letters[i] = 'a' + i;
        i = i + 1;
    }
    return letters[2];
}
