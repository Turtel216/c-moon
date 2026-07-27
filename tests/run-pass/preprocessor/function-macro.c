// A function-like macro substitutes its arguments.
//@ exit-code: 2

#define ADD(a, b) (a + b)

int main() {
    int a = 1;
    int b = 1;

    return ADD(a, b);
}
