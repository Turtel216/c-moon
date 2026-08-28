// C lets a function be declared as often as the program likes, as long as
// every declaration describes the same function.
//@ exit-code: 9

extern int cube(int x);
int cube(int x);
extern int cube(int x);

int cube(int x) {
    return x * x * x;
}

int main() {
    return cube(2) + 1;
}
