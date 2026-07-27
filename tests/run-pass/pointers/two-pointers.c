// Two live pointers into distinct locals.
//@ exit-code: 42

int main() {
    int a = 30;
    int b = 12;
    int *p = &a;
    int *q = &b;
    return *p + *q;
}
