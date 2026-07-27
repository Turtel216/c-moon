// The System V ABI passes the first six integers in registers; arguments
// seven and eight go on the stack.
//@ exit-code: 36

int sum(int a, int b, int c, int d, int e, int f, int g, int h) {
    return a + b + c + d + e + f + g + h;
}

int main() {
    return sum(1, 2, 3, 4, 5, 6, 7, 8);
}
