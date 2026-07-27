// Fill an array in a loop and sum it: 1 + 2 + 3 + 4 + 5.
//@ exit-code: 15

int main() {
    int arr[5];
    int i = 0;
    while (i < 5) {
        arr[i] = i + 1;
        i = i + 1;
    }
    int sum = arr[0] + arr[1] + arr[2] + arr[3] + arr[4];
    return sum;
}
