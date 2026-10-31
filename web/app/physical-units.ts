// Preserve micrometre precision through user entry; canvas rounding never supplies admission data.
export function millimetres(um: bigint): string {
  const sign=um<0n?'-':'';
  const magnitude=um<0n?-um:um;
  const integer = magnitude / 1000n;
  const fraction = (magnitude % 1000n).toString().padStart(3, '0').replace(/0+$/, '');
  return sign+(fraction ? `${integer}.${fraction}` : integer.toString());
}
export function micrometres(mm: string): bigint {
  if (!/^(0|[1-9][0-9]{0,12})(\.[0-9]{1,3})?$/.test(mm)) {
    throw Error('Enter a positive measurement in millimetres with at most three decimal places.');
  }
  const [integer, fraction] = mm.split('.');
  const value = BigInt(integer!) * 1000n + BigInt((fraction === undefined ? '' : fraction).padEnd(3, '0'));
  if (value <= 0n) throw Error('A measurement must be greater than zero. Leave unknown dimensions unset.');
  return value;
}
// A one-sided lower bound cannot reserve a safe maximum envelope.
export function upperMicrometres(quantity:{upper?:bigint;unit:string;scale:number}):bigint|undefined {
  if(quantity.upper===undefined||quantity.unit!=='mm')return undefined;
  if(!Number.isInteger(quantity.scale)||quantity.scale<0||quantity.scale>6||quantity.upper<0n)throw Error('Invalid equipment quantity.');
  if(quantity.scale<=3)return quantity.upper*10n**BigInt(3-quantity.scale);
  const divisor=10n**BigInt(quantity.scale-3);
  return quantity.upper/divisor+(quantity.upper%divisor===0n?0n:1n);
}
