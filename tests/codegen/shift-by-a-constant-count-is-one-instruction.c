// A count the compiler already knows encodes into the shift itself, so nothing
// has to be moved through CL -- the one register a computed count can travel
// in -- and nothing that was living there has to be saved and put back.

// CHECK-LABEL: main:
// CHECK: sar e
// CHECK-NOT: cl

int main() {
    int a = 0;
    a = a + 64;
    return a >> 3;
}
