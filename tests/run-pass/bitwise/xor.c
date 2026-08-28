// Exclusive or keeps the bits exactly one operand has set, and applying the
// same mask twice gives the value back.
//@ exit-code: 6

int main() {
    int a = 12;
    int b = 10;
    int once = a ^ b;

    if ((once ^ b) != a) {
        return 1;
    }
    return once;
}
