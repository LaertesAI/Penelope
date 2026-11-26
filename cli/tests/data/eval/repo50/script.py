import argparse
p=argparse.ArgumentParser()
p.add_argument('--x',type=int,required=True)
a=p.parse_args()
print(a.x*2)
