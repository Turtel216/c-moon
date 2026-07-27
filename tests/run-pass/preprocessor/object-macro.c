// An object-like macro expands at each use site.
//@ exit-code: 15

#define x 5

int main() {
    int a = x;
    int b = x;

    return a + b + x;
}
