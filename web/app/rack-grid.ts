import {PhysicalRackDirection as Direction,type PhysicalRackGrid} from '../src/generated/omloom/v1/workspace';
const pitch=44450n,half=22225n;
const min=-(1n<<63n),max=(1n<<63n)-1n;
export function railGrid(grid:PhysicalRackGrid){
  if(!Number.isInteger(grid.unitCount)||grid.unitCount<1||grid.unitCount>0xffffffff||!Number.isInteger(grid.firstUnit)||grid.firstUnit<1||grid.firstUnit+grid.unitCount-1>0xffffffff)throw Error('Enter a positive rack unit count and numbering range within uint32.');
  if(grid.direction!==Direction.TOP_DOWN&&grid.direction!==Direction.BOTTOM_UP)throw Error('Select the sourced rack numbering direction.');
  const end=grid.topUm+BigInt(grid.unitCount)*pitch;
  if(grid.topUm<min||end>max)throw Error('The rail grid exceeds the coordinate range.');
  return {start:grid.topUm,end,first:BigInt(grid.firstUnit)*2n,halves:BigInt(grid.unitCount)*2n};
}
function halfUnits(unit:string){
  if(!/^[1-9][0-9]{0,9}(\.5)?$/.test(unit))throw Error('Enter a printed rack unit or half-unit position.');
  return BigInt(unit.split('.')[0]!)*2n+(unit.endsWith('.5')?1n:0n);
}
function heightInHalves(height:bigint){if(height<=0n)throw Error('Known positive equipment height is required.');return height/half+(height%half===0n?0n:1n);}
function printed(halves:bigint){return `${halves/2n}${halves%2n===0n?'':'.5'}`;}
// The U editor proposes a conservative half-U allocation, not invented body dimensions.
export function rackAllocation(grid:PhysicalRackGrid,unit:string,height:bigint){
  const rail=railGrid(grid),offset=halfUnits(unit)-rail.first,length=heightInHalves(height)*half;
  if(offset<0n||offset*half+length>rail.end-rail.start)throw Error('The equipment allocation extends outside the declared rack unit range.');
  return {startUm:grid.direction===Direction.TOP_DOWN?rail.start+offset*half:rail.end-offset*half-length,lengthUm:length};
}
export function rackUnitAt(grid:PhysicalRackGrid,y:bigint,height:bigint){
  const rail=railGrid(grid);
  if(y<rail.start||y>=rail.end)return undefined;
  const row=(y-rail.start)/pitch;
  const position=grid.direction===Direction.TOP_DOWN?rail.first+row*2n:rail.first+rail.halves-row*2n-heightInHalves(height);
  if(position<rail.first)return undefined;
  const unit=printed(position);
  rackAllocation(grid,unit,height);
  return unit;
}
export function adjacentRackUnit(grid:PhysicalRackGrid,unit:string,height:bigint,up:boolean){
  const delta=(grid.direction===Direction.TOP_DOWN?up:!up)?-2n:2n;
  const position=halfUnits(unit)+delta;
  if(position<1n)throw Error('The selected rack unit is outside the declared range.');
  const next=printed(position);rackAllocation(grid,next,height);return next;
}
