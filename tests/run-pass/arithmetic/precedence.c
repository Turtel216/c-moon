// Multiplication binds tighter than addition and subtraction.
//@ exit-code: 12

int main() {
    int a = 1;
    int b = 2;
    int c = 2;
    return c * b + 10 - a * 2;
}
