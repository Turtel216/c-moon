// The System V ABI gives every argument a whole word on the stack, so a call
// mixing `int` and `long int` arguments beyond the six passed in registers
// still finds each one where the callee looks for it.
//@ exit-code: 90

long int combine(long int a, int b, long int c, int d, long int e, int f, long int g, int h) {
    return a - b + c - d + e - f + g - h;
}

int main() {
    return combine(10, 1, 20, 2, 30, 3, 40, 4);
}
